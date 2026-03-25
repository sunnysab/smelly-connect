use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::io::{Read, Write};

use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use reqwest::header::{CONTENT_TYPE, COOKIE, USER_AGENT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::EasyConnectConfig;
use crate::error::{BootstrapError, Error};
use crate::resolver::SessionResolver;
use crate::resource::{ResourceSet, parse_resources};
use crate::session::EasyConnectSession;
use crate::transport::{TransportStack, VpnStream};

use super::{encrypt_password, parse_login_auth};

#[derive(Clone)]
pub struct ControlPlaneState {
    pub authorized_twfid: String,
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

    let authorized_twfid = crate::protocol::parse_login_psw_success(&login_psw_body)
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
        resources,
        token: None,
    })
}

pub fn request_token(server: &str, twfid: &str) -> Result<crate::protocol::DerivedToken, Error> {
    let mut builder = SslConnector::builder(SslMethod::tls_client())
        .map_err(|err| Error::Bootstrap(BootstrapError::AuthFlowFailed(err.to_string())))?;
    builder.set_verify(SslVerifyMode::NONE);
    let connector = builder.build();

    let tcp = std::net::TcpStream::connect(server)
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
