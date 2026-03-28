use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};

use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use smelly_tls::{ClientHelloConfig, TunnelConnection};

use crate::config::EasyConnectConfig;
use crate::error::{Error, TunnelBootstrapError};
use crate::transport::device::PacketDevice;

pub type ControlPlaneState = crate::runtime::control_plane::ControlPlaneState;

#[allow(dead_code)]
pub(crate) async fn run_control_plane(
    config: &EasyConnectConfig,
) -> Result<ControlPlaneState, Error> {
    crate::runtime::control_plane::run_control_plane(config).await
}

pub fn request_token(server: &str, twfid: &str) -> Result<crate::protocol::DerivedToken, Error> {
    let mut builder = SslConnector::builder(SslMethod::tls_client()).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    builder.set_verify(SslVerifyMode::NONE);
    let connector = builder.build();

    let tcp_target = if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:443")
    };
    let tcp = std::net::TcpStream::connect(&tcp_target).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    let domain = server.split(':').next().unwrap_or(server);
    let mut stream = connector.connect(domain, tcp).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;

    let request = format!(
        "GET /por/conf.csp HTTP/1.1\r\nHost: {server}\r\nCookie: TWFID={twfid}\r\n\r\nGET /por/rclist.csp HTTP/1.1\r\nHost: {server}\r\nCookie: TWFID={twfid}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    let mut probe = [0_u8; 8];
    let _ = stream.read(&mut probe).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    let session = stream.ssl().session().ok_or_else(|| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(
            "missing SSL session".to_string(),
        ))
    })?;
    crate::protocol::derive_token(&hex::encode(session.id()), twfid).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!("{err:?}")))
    })
}

pub async fn request_token_async(
    server: &str,
    twfid: &str,
) -> Result<crate::protocol::DerivedToken, Error> {
    let server = server.to_string();
    let twfid = twfid.to_string();
    tokio::task::spawn_blocking(move || request_token(&server, &twfid))
        .await
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!(
                "token task join failed: {err}"
            )))
        })?
}

pub async fn request_ip_via_tunnel(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<&str>,
) -> Result<Ipv4Addr, Error> {
    let (ip, _conn) = request_ip_via_tunnel_with_conn(addr, token, legacy_cipher_hint).await?;
    Ok(ip)
}

pub(crate) async fn request_ip_via_tunnel_with_conn(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<&str>,
) -> Result<(Ipv4Addr, TunnelConnection), Error> {
    let request_ip = crate::protocol::build_request_ip_message(token);
    let mut conn = connect_legacy_tunnel(addr, legacy_cipher_hint).await?;
    conn.send_application_data(&request_ip)
        .await
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
        })?;
    let reply = conn.read_application_data().await.map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    let ip = crate::protocol::parse_assigned_ip_reply(&reply).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!("{err:?}")))
    })?;
    Ok((ip, conn))
}

pub async fn request_ip_for_server(
    server: &str,
    token: &crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<&str>,
) -> Result<Ipv4Addr, Error> {
    let addr = resolve_server_addr(server)?;
    request_ip_via_tunnel(addr, token, legacy_cipher_hint).await
}

pub async fn open_recv_tunnel(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    client_ip: Ipv4Addr,
    legacy_cipher_hint: Option<&str>,
) -> Result<TunnelConnection, Error> {
    open_stream_tunnel(
        addr,
        crate::protocol::build_recv_handshake(token, client_ip),
        0x01,
        legacy_cipher_hint,
    )
    .await
}

pub async fn open_send_tunnel(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    client_ip: Ipv4Addr,
    legacy_cipher_hint: Option<&str>,
) -> Result<TunnelConnection, Error> {
    open_stream_tunnel(
        addr,
        crate::protocol::build_send_handshake(token, client_ip),
        0x02,
        legacy_cipher_hint,
    )
    .await
}

pub async fn spawn_legacy_packet_device(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    client_ip: Ipv4Addr,
    legacy_cipher_hint: Option<&str>,
) -> Result<PacketDevice, Error> {
    let recv = open_recv_tunnel(addr, token, client_ip, legacy_cipher_hint).await?;
    let send = open_send_tunnel(addr, token, client_ip, legacy_cipher_hint).await?;

    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(128);
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(128);
    let mut device = PacketDevice::new(inbound_tx.clone(), inbound_rx, outbound_tx, outbound_rx);
    let mut outbound_rx = device.take_outbound_rx().ok_or_else(|| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(
            "missing outbound rx".to_string(),
        ))
    })?;

    tokio::spawn(async move {
        let mut recv = recv;
        while let Ok(packet) = recv.read_application_data().await {
            let _ = inbound_tx.send(packet).await;
        }
    });

    tokio::spawn(async move {
        let mut send = send;
        while let Some(packet) = outbound_rx.recv().await {
            let _ = send.send_application_data(&packet).await;
        }
    });

    Ok(device)
}

pub(crate) fn resolve_server_addr(server: &str) -> Result<SocketAddr, Error> {
    let target = if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:443")
    };
    target
        .to_socket_addrs()
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
        })?
        .next()
        .ok_or_else(|| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(
                "no resolved address".to_string(),
        ))
    })
}

pub(crate) async fn resolve_server_addr_async(server: &str) -> Result<SocketAddr, Error> {
    let server = server.to_string();
    tokio::task::spawn_blocking(move || resolve_server_addr(&server))
        .await
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!(
                "server resolution task join failed: {err}"
            )))
        })?
}

async fn open_stream_tunnel(
    addr: SocketAddr,
    handshake: Vec<u8>,
    expected_reply_type: u8,
    legacy_cipher_hint: Option<&str>,
) -> Result<TunnelConnection, Error> {
    let mut conn = connect_legacy_tunnel(addr, legacy_cipher_hint).await?;
    conn.send_application_data(&handshake)
        .await
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
        })?;
    let reply = conn.read_application_data().await.map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    let actual = reply.first().copied().unwrap_or_default();
    if actual != expected_reply_type {
        return Err(Error::TunnelBootstrap(
            TunnelBootstrapError::HandshakeFailed(format!(
                "unexpected stream handshake reply: got 0x{actual:02x}, want 0x{expected_reply_type:02x}"
            )),
        ));
    }
    Ok(conn)
}

async fn connect_legacy_tunnel(
    addr: SocketAddr,
    legacy_cipher_hint: Option<&str>,
) -> Result<TunnelConnection, Error> {
    let mut last_err = None;
    for cipher_suite in crate::kernel::tunnel::cipher_suite_attempts(legacy_cipher_hint) {
        let hello = ClientHelloConfig::new(
            [0x41; 32],
            *b"L3IP\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        )
        .with_cipher_suite(cipher_suite)
        .with_compression_methods(vec![1, 0]);

        match smelly_tls::connect_tunnel(addr, &hello).await {
            Ok(conn) => return Ok(conn),
            Err(err) => last_err = Some(err.to_string()),
        }
    }

    Err(Error::TunnelBootstrap(
        TunnelBootstrapError::HandshakeFailed(
            last_err.unwrap_or_else(|| "legacy tunnel failed".to_string()),
        ),
    ))
}
