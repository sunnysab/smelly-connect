use crate::error::{ControlPlaneError, Error};
use smelly_tls::ServerCertPolicy;

fn apply_server_cert_policy(
    mut builder: reqwest::ClientBuilder,
    server_cert_policy: &ServerCertPolicy,
) -> Result<reqwest::ClientBuilder, Error> {
    match server_cert_policy {
        ServerCertPolicy::Verify => {}
        ServerCertPolicy::VerifyWithCustomRoots(custom_roots) => {
            for cert_der in custom_roots {
                let cert = reqwest::Certificate::from_der(cert_der).map_err(|err| {
                    Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string()))
                })?;
                builder = builder.add_root_certificate(cert);
            }
        }
        ServerCertPolicy::InsecureSkipVerify => {
            builder = builder
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
    }

    Ok(builder)
}

pub(crate) fn build_reqwest_client(
    server_cert_policy: &ServerCertPolicy,
) -> Result<reqwest::Client, Error> {
    apply_server_cert_policy(reqwest::Client::builder(), server_cert_policy)?
        .build()
        .map_err(|err| Error::ControlPlane(ControlPlaneError::AuthFlowFailed(err.to_string())))
}

#[cfg(test)]
mod tests {
    #[allow(dead_code)]
    mod legacy_tls {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-support/legacy_tls.rs"
        ));
    }

    use std::sync::{Arc, Once};

    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use smelly_tls::ServerCertPolicy;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsAcceptor;

    use super::{apply_server_cert_policy, build_reqwest_client};

    fn install_rustls_provider() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn tls_acceptor() -> TlsAcceptor {
        install_rustls_provider();
        let certs = vec![CertificateDer::from(legacy_tls::server_certificate_der())];
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
            legacy_tls::server_private_key_der(),
        ));
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("valid tls server config");
        TlsAcceptor::from(Arc::new(config))
    }

    async fn spawn_https_server(expected_connections: usize) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = tls_acceptor();

        tokio::spawn(async move {
            for _ in 0..expected_connections {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut buffer = Vec::new();
                    loop {
                        let mut chunk = [0_u8; 1024];
                        let Ok(n) = tls.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buffer.extend_from_slice(&chunk[..n]);
                        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let response =
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                    let _ = tls.write_all(response).await;
                    let _ = tls.flush().await;
                });
            }
        });

        addr
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reqwest_client_accepts_custom_root_certificates() {
        let addr = spawn_https_server(1).await;
        let url = format!("https://127.0.0.1:{}/", addr.port());
        install_rustls_provider();
        let client = build_reqwest_client(&ServerCertPolicy::VerifyWithCustomRoots(vec![
            legacy_tls::root_certificate_der(),
        ]))
        .unwrap();

        let body = client.get(url).send().await.unwrap().text().await.unwrap();

        assert_eq!(body, "ok");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reqwest_client_rejects_unknown_roots_unless_insecure_mode_is_enabled() {
        let addr = spawn_https_server(2).await;
        let url = format!("https://127.0.0.1:{}/", addr.port());
        install_rustls_provider();

        let strict_err = build_reqwest_client(&ServerCertPolicy::Verify)
            .unwrap()
            .get(&url)
            .send()
            .await
            .unwrap_err();
        let _ = strict_err;

        let body = build_reqwest_client(&ServerCertPolicy::InsecureSkipVerify)
            .unwrap()
            .get(url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "ok");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reqwest_client_rejects_wrong_host_unless_insecure_mode_is_enabled() {
        let addr = spawn_https_server(2).await;
        let url = format!("https://vpn.example.com:{}/", addr.port());
        install_rustls_provider();

        let strict_err = apply_server_cert_policy(
            reqwest::Client::builder().resolve("vpn.example.com", addr),
            &ServerCertPolicy::VerifyWithCustomRoots(vec![legacy_tls::root_certificate_der()]),
        )
        .unwrap()
        .build()
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap_err();
        let _ = strict_err;

        let body = apply_server_cert_policy(
            reqwest::Client::builder().resolve("vpn.example.com", addr),
            &ServerCertPolicy::InsecureSkipVerify,
        )
        .unwrap()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
        assert_eq!(body, "ok");
    }
}
