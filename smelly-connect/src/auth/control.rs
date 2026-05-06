use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};

use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use smelly_tls::{ClientHelloConfig, TunnelConnection};
use tracing::info;

use crate::config::EasyConnectConfig;
use crate::error::{Error, TunnelBootstrapError};
use crate::kernel::tunnel::command::{
    FIXED_SSL_ACK, FIXED_SSL_SYN, SERVER_MSG_LEN, SERVER_SSL_ACK_LEN, SendIpInfo, ServerMsg,
    build_new_connect_msg, derive_peer_sockaddr,
};
use crate::kernel::tunnel::dataplane::DataTunnel;
use crate::transport::device::PacketDevice;

pub type ControlPlaneState = crate::runtime::control_plane::ControlPlaneState;

#[allow(dead_code)]
pub(crate) async fn run_control_plane(
    config: &EasyConnectConfig,
) -> Result<ControlPlaneState, Error> {
    crate::runtime::control_plane::run_control_plane(config).await
}

/// Legacy: Derive token from TLS ServerHello SessionID.
/// Used when sslctx is not available (old protocol path).
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

pub async fn request_ip_via_tunnel_with_conn_debug(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<&str>,
) -> Result<(Ipv4Addr, TunnelConnection), Error> {
    request_ip_via_tunnel_with_conn(addr, token, legacy_cipher_hint).await
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

/// Legacy: Open a recv data tunnel (TLS handshake + 0x06 message).
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

    packet_device_from_tunnels(recv, send)
}

pub(crate) fn packet_device_from_tunnels(
    recv: TunnelConnection,
    send: TunnelConnection,
) -> Result<PacketDevice, Error> {
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(1024);
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(1024);
    let mut device = PacketDevice::new(inbound_tx.clone(), inbound_rx, outbound_tx, outbound_rx);
    let mut outbound_rx = device.take_outbound_rx().ok_or_else(|| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(
            "missing outbound rx".to_string(),
        ))
    })?;

    tokio::spawn(async move {
        let mut recv = recv;
        while let Ok(packet) = recv.read_application_data().await {
            log_packet("vpn->stack", &packet);
            let _ = inbound_tx.send(packet).await;
        }
    });

    tokio::spawn(async move {
        let mut send = send;
        while let Some(packet) = outbound_rx.recv().await {
            log_packet("stack->vpn", &packet);
            let _ = send.send_application_data(&packet).await;
        }
    });

    Ok(device)
}

fn log_packet(direction: &str, packet: &[u8]) {
    let description = describe_ipv4_packet(packet)
        .unwrap_or_else(|| format!("len={} non-ipv4", packet.len()));
    info!(direction, packet = %description, "legacy tunnel packet");
}

fn describe_ipv4_packet(packet: &[u8]) -> Option<String> {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return None;
    }
    let ihl = (packet[0] & 0x0f) as usize * 4;
    if packet.len() < ihl || ihl < 20 {
        return None;
    }
    let proto = packet[9];
    let src = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let dst = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    Some(format!(
        "len={} proto={} src={} dst={}",
        packet.len(),
        proto,
        src,
        dst
    ))
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

// ============================================================
// Legacy TLS protocol (pre-sslctx, kept for backward compat)
// ============================================================

/// Legacy: Open a TLS tunnel with a handshake message and validate reply type.
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

// ============================================================
// Command tunnel protocol (JJYY/AABB) — new protocol
// ============================================================

/// Perform the fixed TLS-looking handshake on a TCP connection.
/// Sends FixedSSLSyn, reads 0x7A bytes, sends FixedSSLAck.
fn perform_fixed_tls_handshake(stream: &mut std::net::TcpStream) -> Result<(), Error> {
    stream.write_all(FIXED_SSL_SYN).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    let mut server_ack = [0u8; SERVER_SSL_ACK_LEN];
    stream.read_exact(&mut server_ack).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    stream.write_all(FIXED_SSL_ACK).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;
    Ok(())
}

/// Connect to the command tunnel, send NEWCONNECT, and parse the SEND_IP response.
/// Returns (SendIpInfo, command_tunnel_stream).
/// The command tunnel stream MUST be kept alive for heartbeats.
pub fn connect_command_tunnel(
    addr: SocketAddr,
) -> Result<(SendIpInfo, std::net::TcpStream), Error> {
    let mut stream = std::net::TcpStream::connect(addr).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;

    perform_fixed_tls_handshake(&mut stream)?;

    let peer = derive_peer_sockaddr(&addr.ip().to_string(), addr.port());
    let msg = build_new_connect_msg(&peer);
    stream.write_all(&msg).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;

    let mut reply = [0u8; SERVER_MSG_LEN];
    stream.read_exact(&mut reply).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(err.to_string()))
    })?;

    let server_msg = ServerMsg::parse(&reply).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!("{err}")))
    })?;

    let send_ip = SendIpInfo::from_server_msg(&server_msg).map_err(|err| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!("{err}")))
    })?;

    Ok((send_ip, stream))
}

/// Async wrapper for `connect_command_tunnel`.
pub async fn connect_command_tunnel_async(
    addr: SocketAddr,
) -> Result<(SendIpInfo, std::net::TcpStream), Error> {
    tokio::task::spawn_blocking(move || connect_command_tunnel(addr))
        .await
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!(
                "command tunnel task join failed: {err}"
            )))
        })?
}

/// Connect send and recv data tunnels using the command tunnel protocol.
/// Returns a PacketDevice that bridges VPN tunnels with the smoltcp stack.
pub fn connect_data_tunnels(
    addr: SocketAddr,
    peer_sockaddr: &[u8; 16],
    tun_ip: Ipv4Addr,
    rc4_key: &[u8; 16],
) -> Result<PacketDevice, Error> {
    let recv = crate::kernel::tunnel::connect_recv_tunnel(addr, peer_sockaddr, tun_ip, rc4_key)
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!(
                "recv tunnel: {err}"
            )))
        })?;
    let send = crate::kernel::tunnel::connect_send_tunnel(addr, peer_sockaddr, tun_ip, rc4_key)
        .map_err(|err| {
            Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(format!(
                "send tunnel: {err}"
            )))
        })?;

    packet_device_from_data_tunnels(recv, send)
}

/// Create a PacketDevice from two DataTunnels (recv + send).
/// Spawns tasks for VPN↔stack packet bridging with IPCP framing.
pub fn packet_device_from_data_tunnels(
    recv: DataTunnel,
    send: DataTunnel,
) -> Result<PacketDevice, Error> {
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(1024);
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(1024);
    let mut device = PacketDevice::new(inbound_tx.clone(), inbound_rx, outbound_tx, outbound_rx);
    let mut outbound_rx = device.take_outbound_rx().ok_or_else(|| {
        Error::TunnelBootstrap(TunnelBootstrapError::HandshakeFailed(
            "missing outbound rx".to_string(),
        ))
    })?;

    let (recv_stream, recv_rc4) = recv.into_parts();
    let (send_stream, send_rc4) = send.into_parts();

    // Recv tunnel → IPCP decode → inbound_tx (VPN → stack)
    tokio::spawn(async move {
        loop {
            let packet = tokio::task::spawn_blocking({
                let mut stream = recv_stream.try_clone().expect("tcp stream clone failed");
                let mut rc4 = recv_rc4.clone();
                move || crate::kernel::tunnel::read_ipcp_frame(&mut stream, &mut rc4)
            })
            .await;

            match packet {
                Ok(Ok(packet)) => {
                    log_packet("vpn->stack", &packet);
                    if inbound_tx.send(packet).await.is_err() {
                        break;
                    }
                }
                Ok(Err(err)) => {
                    tracing::error!("recv tunnel IPCP decode error: {err}");
                    break;
                }
                Err(err) => {
                    tracing::error!("recv tunnel task error: {err}");
                    break;
                }
            }
        }
    });

    // Stack → IPCP encode → send tunnel (stack → VPN)
    tokio::spawn(async move {
        while let Some(packet) = outbound_rx.recv().await {
            log_packet("stack->vpn", &packet);
            let result = tokio::task::spawn_blocking({
                let mut stream = send_stream.try_clone().expect("tcp stream clone failed");
                let mut rc4 = send_rc4.clone();
                move || {
                    let frame = crate::kernel::tunnel::encode_ipcp(&packet, 0, 0, &mut rc4);
                    stream.write_all(&frame).map_err(std::io::Error::from)
                }
            })
            .await;

            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::error!("send tunnel IPCP encode error: {err}");
                    break;
                }
                Err(err) => {
                    tracing::error!("send tunnel task error: {err}");
                    break;
                }
            }
        }
    });

    Ok(device)
}
