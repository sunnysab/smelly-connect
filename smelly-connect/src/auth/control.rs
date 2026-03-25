use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use reqwest::header::{CONTENT_TYPE, COOKIE, USER_AGENT};
use smelly_tls::{ClientHelloConfig, TunnelConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::EasyConnectConfig;
use crate::error::{BootstrapError, Error};
use crate::resolver::SessionResolver;
use crate::resource::{ResourceSet, parse_resources};
use crate::session::EasyConnectSession;
use crate::transport::device::PacketDevice;
use crate::transport::{TransportStack, VpnStream};

use super::{encrypt_password, parse_login_auth};

#[derive(Clone)]
pub struct ControlPlaneState {
    pub authorized_twfid: String,
    pub legacy_cipher_hint: Option<String>,
    pub resources: ResourceSet,
    pub token: Option<crate::protocol::DerivedToken>,
}

pub async fn run_control_plane(config: &EasyConnectConfig) -> Result<ControlPlaneState, Error> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let base_url = config.control_base_url();

    let login_auth_body = client
        .get(format!("{base_url}/por/login_auth.csp?apiversion=1"))
        .send()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?
        .text()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;

    let parsed = parse_login_auth(&login_auth_body)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(format!("{err:?}"))))?;

    let mut rand_code = String::new();
    if parsed.requires_captcha {
        let captcha_handler = config
            .captcha_handler
            .clone()
            .ok_or(Error::Bootstrap(BootstrapError::CaptchaRequired))?;
        let response = client
            .get(format!("{base_url}/por/rand_code.csp?apiversion=1"))
            .header(COOKIE, format!("TWFID={}", parsed.twfid))
            .header(USER_AGENT, "EasyConnect_windows")
            .send()
            .await
            .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
        let mime_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = response
            .bytes()
            .await
            .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
        rand_code = captcha_handler
            .solve(bytes.to_vec(), mime_type)
            .await
            .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    }

    let encrypted_password = encrypt_password(
        &config.password,
        parsed.csrf_rand_code.as_deref(),
        &parsed.rsa_key_hex,
        parsed.rsa_exp,
    )
    .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(format!("{err:?}"))))?;

    let mut form = HashMap::new();
    form.insert("svpn_rand_code", rand_code);
    form.insert("mitm", String::new());
    form.insert(
        "svpn_req_randcode",
        parsed.csrf_rand_code.clone().unwrap_or_default(),
    );
    form.insert("svpn_name", config.username.clone());
    form.insert("svpn_password", encrypted_password);

    let login_psw_body = client
        .post(format!("{base_url}/por/login_psw.csp?anti_replay=1&encrypt=1&type=cs"))
        .header(COOKIE, format!("TWFID={}", parsed.twfid))
        .header(USER_AGENT, "EasyConnect_windows")
        .form(&form)
        .send()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?
        .text()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;

    let authorized_twfid = crate::protocol::parse_login_psw_success(&login_psw_body, &parsed.twfid)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(format!("{err:?}"))))?;

    let resource_body = client
        .get(format!("{base_url}/por/rclist.csp"))
        .header(COOKIE, format!("TWFID={authorized_twfid}"))
        .send()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?
        .text()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;

    let resources = parse_resources(&resource_body)
        .map_err(|err| Error::Bootstrap(BootstrapError::ResourceParseFailed(err.to_string())))?;

    Ok(ControlPlaneState {
        authorized_twfid,
        legacy_cipher_hint: parsed.legacy_cipher_hint,
        resources,
        token: None,
    })
}

pub fn request_token(server: &str, twfid: &str) -> Result<crate::protocol::DerivedToken, Error> {
    let mut builder = SslConnector::builder(SslMethod::tls_client())
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    builder.set_verify(SslVerifyMode::NONE);
    let connector = builder.build();

    let tcp_target = if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:443")
    };
    let tcp = std::net::TcpStream::connect(&tcp_target)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let domain = server.split(':').next().unwrap_or(server);
    let mut stream = connector
        .connect(domain, tcp)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;

    let request = format!(
        "GET /por/conf.csp HTTP/1.1\r\nHost: {server}\r\nCookie: TWFID={twfid}\r\n\r\nGET /por/rclist.csp HTTP/1.1\r\nHost: {server}\r\nCookie: TWFID={twfid}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let mut probe = [0_u8; 8];
    let _ = stream
        .read(&mut probe)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let session = stream
        .ssl()
        .session()
        .ok_or_else(|| Error::Bootstrap(BootstrapError::AuthFlowFailed("missing SSL session".to_string())))?;
    crate::protocol::derive_token(&hex::encode(session.id()), twfid)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(format!("{err:?}"))))
}

pub async fn request_ip_via_tunnel(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<&str>,
) -> Result<Ipv4Addr, Error> {
    let (ip, _conn) = request_ip_via_tunnel_with_conn(addr, token, legacy_cipher_hint).await?;
    Ok(ip)
}

pub async fn request_ip_via_tunnel_with_conn(
    addr: SocketAddr,
    token: &crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<&str>,
) -> Result<(Ipv4Addr, TunnelConnection), Error> {
    let request_ip = crate::protocol::build_request_ip_message(token);
    let mut conn = connect_legacy_tunnel(addr, legacy_cipher_hint).await?;
    conn.send_application_data(&request_ip)
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let reply = conn
        .read_application_data()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let ip = crate::protocol::parse_assigned_ip_reply(&reply)
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(format!("{err:?}"))))?;
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
    let mut outbound_rx = device
        .take_outbound_rx()
        .ok_or_else(|| Error::Bootstrap(BootstrapError::AuthFlowFailed("missing outbound rx".to_string())))?;

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
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?
        .next()
        .ok_or_else(|| Error::Bootstrap(BootstrapError::AuthFlowFailed("no resolved address".to_string())))
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
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let reply = conn
        .read_application_data()
        .await
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    let actual = reply.first().copied().unwrap_or_default();
    if actual != expected_reply_type {
        return Err(Error::Bootstrap(BootstrapError::AuthFlowFailed(format!(
            "unexpected stream handshake reply: got 0x{actual:02x}, want 0x{expected_reply_type:02x}"
        ))));
    }
    Ok(conn)
}

async fn connect_legacy_tunnel(
    addr: SocketAddr,
    legacy_cipher_hint: Option<&str>,
) -> Result<TunnelConnection, Error> {
    let preferred = legacy_cipher_hint
        .and_then(smelly_tls::legacy_cipher_suite_from_hint)
        .unwrap_or(smelly_tls::TLS_RSA_WITH_RC4_128_SHA);
    let mut attempts = vec![preferred];
    if preferred != smelly_tls::TLS_RSA_WITH_RC4_128_SHA {
        attempts.push(smelly_tls::TLS_RSA_WITH_RC4_128_SHA);
    }

    let mut last_err = None;
    for cipher_suite in attempts {
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

    Err(Error::Bootstrap(BootstrapError::AuthFlowFailed(
        last_err.unwrap_or_else(|| "legacy tunnel failed".to_string()),
    )))
}

pub mod tests {
    use super::*;

    pub struct ControlPlaneHarness {
        base_url: String,
    }

    impl ControlPlaneHarness {
        pub fn config(&self) -> EasyConnectConfig {
            let captcha = crate::CaptchaHandler::from_async(|bytes, mime| async move {
                assert_eq!(bytes, vec![1, 2, 3, 4]);
                assert_eq!(mime.as_deref(), Some("image/jpeg"));
                Ok::<_, crate::CaptchaError>("1234".to_string())
            });
            EasyConnectConfig::new("rvpn.example.com", "user", "pass")
                .with_base_url(self.base_url.clone())
                .with_captcha_handler(captcha)
                .with_session_bootstrap(|state| {
                    let ip = Ipv4Addr::new(10, 0, 0, 8);
                    let mut system_dns = HashMap::new();
                    for host in state.resources.domain_rules.keys() {
                        system_dns.insert(host.clone(), IpAddr::V4(ip));
                    }
                    for (host, resolved) in &state.resources.static_dns {
                        system_dns.insert(host.clone(), *resolved);
                    }
                    let transport = TransportStack::new(|_| async move {
                        let (client, _server) = tokio::io::duplex(1024);
                        Ok(VpnStream::new(client))
                    });
                    Ok(EasyConnectSession::new(
                        ip,
                        state.resources,
                        SessionResolver::new(HashMap::new(), None, system_dns),
                        transport,
                    ))
                })
        }
    }

    pub async fn control_plane_harness() -> ControlPlaneHarness {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let request = read_request(&mut stream).await.unwrap();
                    let first_line = request.lines().next().unwrap_or_default().to_string();
                    if first_line.starts_with("GET /por/login_auth.csp?apiversion=1") {
                        write_response(
                            &mut stream,
                            "200 OK",
                            "text/xml",
                            include_str!("../../tests/fixtures/login_auth_requires_captcha.xml")
                                .as_bytes(),
                        )
                        .await
                        .unwrap();
                    } else if first_line.starts_with("GET /por/rand_code.csp?apiversion=1") {
                        write_response(&mut stream, "200 OK", "image/jpeg", &[1, 2, 3, 4])
                            .await
                            .unwrap();
                    } else if first_line
                        .starts_with("POST /por/login_psw.csp?anti_replay=1&encrypt=1&type=cs")
                    {
                        let request_lower = request.to_ascii_lowercase();
                        assert!(request_lower.contains("cookie: twfid=dummy-twfid"));
                        assert!(request.contains("svpn_name=user"));
                        assert!(request.contains("svpn_rand_code=1234"));
                        assert!(request.contains("svpn_req_randcode=csrf-1234"));
                        write_response(
                            &mut stream,
                            "200 OK",
                            "text/xml",
                            include_str!("../../tests/fixtures/login_psw_success.xml").as_bytes(),
                        )
                        .await
                        .unwrap();
                    } else if first_line.starts_with("GET /por/rclist.csp") {
                        let request_lower = request.to_ascii_lowercase();
                        assert!(request_lower.contains("cookie: twfid=updated-twfid"));
                        write_response(
                            &mut stream,
                            "200 OK",
                            "text/xml",
                            include_str!("../../tests/fixtures/resource_sample.xml").as_bytes(),
                        )
                        .await
                        .unwrap();
                    } else {
                        write_response(&mut stream, "404 Not Found", "text/plain", b"not found")
                            .await
                            .unwrap();
                    }
                });
            }
        });

        ControlPlaneHarness {
            base_url: format!("http://{addr}"),
        }
    }

    async fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
        let mut buffer = Vec::new();
        let mut header_end = None;
        loop {
            let mut chunk = [0_u8; 1024];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
            if header_end.is_none() {
                header_end = find_header_end(&buffer);
            }
            if let Some(end) = header_end {
                let header_text = String::from_utf8_lossy(&buffer[..end]);
                let content_length = header_text
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Content-Length: ")
                            .and_then(|v| v.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if buffer.len() >= end + content_length {
                    break;
                }
            }
        }
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }

    fn find_header_end(buffer: &[u8]) -> Option<usize> {
        buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|idx| idx + 4)
    }

    async fn write_response(
        stream: &mut TcpStream,
        status: &str,
        content_type: &str,
        body: &[u8],
    ) -> std::io::Result<()> {
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(body).await
    }
}
