pub mod http;

pub mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use crate::resolver::SessionResolver;
    use crate::resource::{DomainRule, IpRule, ResourceSet};
    use crate::session::EasyConnectSession;
    use crate::transport::{TransportStack, VpnStream};

    pub struct HttpProxyHarness {
        proxy_addr: SocketAddr,
        #[allow(dead_code)]
        handle: crate::proxy::http::HttpProxyHandle,
    }

    impl HttpProxyHarness {
        pub async fn get_via_proxy(&self, url: &str) -> String {
            let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
            let request = format!(
                "GET {url} HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n"
            );
            client.write_all(request.as_bytes()).await.unwrap();

            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            let response = String::from_utf8(response).unwrap();
            response.split("\r\n\r\n").nth(1).unwrap().to_string()
        }

        pub async fn connect_tunnel(&self, target: &str) -> Result<(), Box<dyn std::error::Error>> {
            let mut client = TcpStream::connect(self.proxy_addr).await?;
            let request = format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n"
            );
            client.write_all(request.as_bytes()).await?;

            let mut header = vec![0_u8; 128];
            let n = client.read(&mut header).await?;
            let header = String::from_utf8_lossy(&header[..n]);
            assert!(header.starts_with("HTTP/1.1 200"));

            client.write_all(b"ping").await?;
            let mut echoed = [0_u8; 4];
            client.read_exact(&mut echoed).await?;
            assert_eq!(&echoed, b"ping");
            Ok(())
        }
    }

    pub async fn http_proxy_harness() -> HttpProxyHarness {
        let http_upstream = spawn_http_upstream().await;
        let tunnel_upstream = spawn_echo_upstream().await;
        let session = proxy_ready_session(http_upstream, tunnel_upstream);
        let handle = session
            .start_http_proxy("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        HttpProxyHarness {
            proxy_addr: handle.local_addr(),
            handle,
        }
    }

    async fn spawn_http_upstream() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            let _ = socket.read(&mut buf).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        addr
    }

    async fn spawn_echo_upstream() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                socket.write_all(&buf[..n]).await.unwrap();
            }
        });
        addr
    }

    fn proxy_ready_session(http_upstream: SocketAddr, tunnel_upstream: SocketAddr) -> EasyConnectSession {
        let http_host = "intranet.zju.edu.cn";
        let tunnel_host = "libdb.zju.edu.cn";
        let resolved_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        let mut resources = ResourceSet::default();
        resources.domain_rules.insert(
            http_host.to_string(),
            DomainRule {
                port_min: 80,
                port_max: 80,
                protocol: "all".to_string(),
            },
        );
        resources.domain_rules.insert(
            tunnel_host.to_string(),
            DomainRule {
                port_min: 443,
                port_max: 443,
                protocol: "all".to_string(),
            },
        );
        resources.ip_rules.push(IpRule {
            ip_min: resolved_ip,
            ip_max: resolved_ip,
            port_min: 1,
            port_max: 65535,
            protocol: "all".to_string(),
        });

        let mut system_dns = HashMap::new();
        system_dns.insert(http_host.to_string(), resolved_ip);
        system_dns.insert(tunnel_host.to_string(), resolved_ip);

        let transport = TransportStack::new(move |target| {
            let http_upstream = http_upstream;
            let tunnel_upstream = tunnel_upstream;
            async move {
                let upstream = match target.port() {
                    80 => http_upstream,
                    443 => tunnel_upstream,
                    port => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                };
                let stream = TcpStream::connect(upstream).await?;
                Ok(VpnStream::new(stream))
            }
        });

        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            resources,
            SessionResolver::new(HashMap::new(), None, system_dns),
            transport,
        )
    }
}
