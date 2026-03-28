use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::EasyConnectConfig;
use crate::resolver::SessionResolver;
use crate::session::EasyConnectSession;
use crate::transport::{TransportStack, VpnStream};

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
                        include_str!("../../tests/fixtures/login_auth_requires_captcha.xml").as_bytes(),
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
