use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::proxy::http::HttpProxyHandle;
use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::session::EasyConnectSession;
use crate::transport::{TransportStack, VpnStream};

pub struct HttpProxyHarness {
    proxy_addr: SocketAddr,
    #[allow(dead_code)]
    handle: HttpProxyHandle,
}

impl HttpProxyHarness {
    pub async fn get_via_proxy(&self, url: &str) -> String {
        self.get_via_proxy_with_connection(url, "close").await
    }

    pub async fn get_via_proxy_with_connection(&self, url: &str, connection: &str) -> String {
        let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
        let request = format!(
            "GET {url} HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: {connection}\r\n\r\n"
        );
        client.write_all(request.as_bytes()).await.unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), async {
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            response
        })
        .await
        .unwrap();
        let response = String::from_utf8(response).unwrap();
        response.split("\r\n\r\n").nth(1).unwrap().to_string()
    }

    pub async fn connect_tunnel(&self, target: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = TcpStream::connect(self.proxy_addr).await?;
        let request =
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
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

    pub async fn post_split_body_via_proxy(&self, url: &str, first: &str, second: &str) -> String {
        let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
        let request = format!(
            "POST {url} HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{first}",
            first.len() + second.len()
        );
        client.write_all(request.as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        client.write_all(second.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        response.split("\r\n\r\n").nth(1).unwrap().to_string()
    }

    pub async fn post_split_chunked_body_via_proxy(
        &self,
        url: &str,
        first: &str,
        second: &str,
    ) -> String {
        let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
        let request = format!(
            "POST {url} HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{first}"
        );
        client.write_all(request.as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        client.write_all(second.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        response.split("\r\n\r\n").nth(1).unwrap().to_string()
    }

    pub async fn post_expect_continue_via_proxy(&self, url: &str, body: &str) -> String {
        let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
        let request = format!(
            "POST {url} HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        );
        client.write_all(request.as_bytes()).await.unwrap();

        let mut interim = [0_u8; 128];
        let n = tokio::time::timeout(Duration::from_secs(1), client.read(&mut interim))
            .await
            .unwrap()
            .unwrap();
        let interim = String::from_utf8_lossy(&interim[..n]);
        assert!(interim.starts_with("HTTP/1.1 100 Continue"));

        client.write_all(body.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        response.split("\r\n\r\n").nth(1).unwrap().to_string()
    }

    pub async fn get_with_proxy_authorization_via_proxy(
        &self,
        url: &str,
        credentials: &str,
    ) -> String {
        let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
        let request = format!(
            "GET {url} HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nProxy-Authorization: {credentials}\r\nConnection: close\r\n\r\n"
        );
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        response.split("\r\n\r\n").nth(1).unwrap().to_string()
    }

    pub async fn oversized_header_status_via_proxy(&self) -> u16 {
        let mut client = TcpStream::connect(self.proxy_addr).await.unwrap();
        let oversized = "a".repeat(17 * 1024);
        let request = format!(
            "GET http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nX-Oversized: {oversized}\r\nConnection: close\r\n\r\n"
        );
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = [0_u8; 256];
        let n = client.read(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response[..n]);
        response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap()
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

pub async fn http_proxy_harness_with_body_echo() -> HttpProxyHarness {
    let http_upstream = spawn_body_echo_http_upstream().await;
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

pub async fn http_proxy_harness_with_keep_alive() -> HttpProxyHarness {
    let http_upstream = spawn_keep_alive_http_upstream().await;
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

pub async fn http_proxy_harness_with_chunked_body_echo() -> HttpProxyHarness {
    let http_upstream = spawn_chunked_body_echo_http_upstream().await;
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

pub async fn http_proxy_harness_with_proxy_auth_capture() -> HttpProxyHarness {
    let http_upstream = spawn_proxy_auth_capture_http_upstream().await;
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

async fn spawn_body_echo_http_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        let mut content_length = None::<usize>;
        let mut header_end = None::<usize>;
        let mut body_complete = false;

        loop {
            let n = match tokio::time::timeout(Duration::from_millis(200), socket.read(&mut chunk))
                .await
            {
                Ok(Ok(n)) => n,
                Ok(Err(err)) => panic!("upstream read failed: {err}"),
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..n]);
            if header_end.is_none() {
                header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|idx| idx + 4);
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    content_length = headers.lines().find_map(|line| {
                        let lower = line.to_ascii_lowercase();
                        lower
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    });
                }
            }
            if let (Some(end), Some(length)) = (header_end, content_length)
                && request.len() >= end + length
            {
                body_complete = true;
                break;
            }
        }

        let body = if let (Some(end), Some(length)) = (header_end, content_length) {
            let available = request.len().saturating_sub(end).min(length);
            String::from_utf8_lossy(&request[end..end + available]).to_string()
        } else {
            String::new()
        };
        let status = if body_complete {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    addr
}

async fn spawn_keep_alive_http_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 2048];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: keep-alive\r\n\r\nhello",
            )
            .await
            .unwrap();
        if request.to_ascii_lowercase().contains("connection: close") {
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    addr
}

async fn spawn_chunked_body_echo_http_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];

        loop {
            let n = match tokio::time::timeout(Duration::from_millis(200), socket.read(&mut chunk))
                .await
            {
                Ok(Ok(n)) => n,
                Ok(Err(err)) => panic!("upstream read failed: {err}"),
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..n]);
            if chunked_request_complete(&request) {
                break;
            }
        }

        let body = extract_chunked_request_body(&request).unwrap_or_default();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    addr
}

async fn spawn_proxy_auth_capture_http_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..n]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
        let body = if request.contains("proxy-authorization:") {
            "leaked"
        } else {
            "clean"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    addr
}

fn chunked_request_complete(request: &[u8]) -> bool {
    let Some(header_end) = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)
    else {
        return false;
    };
    let body = &request[header_end..];
    chunked_wire_complete(body)
}

fn extract_chunked_request_body(request: &[u8]) -> Option<String> {
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)?;
    let body = &request[header_end..];
    if !chunked_wire_complete(body) {
        return None;
    }
    let mut cursor = 0usize;
    let mut decoded = Vec::new();
    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")?
            + cursor;
        let size_line = std::str::from_utf8(&body[cursor..line_end]).ok()?;
        let size = usize::from_str_radix(size_line.split(';').next()?.trim(), 16).ok()?;
        cursor = line_end + 2;
        if size == 0 {
            return String::from_utf8(decoded).ok();
        }
        decoded.extend_from_slice(body.get(cursor..cursor + size)?);
        cursor += size;
        if body.get(cursor..cursor + 2)? != b"\r\n" {
            return None;
        }
        cursor += 2;
    }
}

fn chunked_wire_complete(body: &[u8]) -> bool {
    let mut cursor = 0usize;
    loop {
        let Some(line_rel_end) = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
        else {
            return false;
        };
        let line_end = cursor + line_rel_end;
        let Ok(size_line) = std::str::from_utf8(&body[cursor..line_end]) else {
            return false;
        };
        let Ok(size) = usize::from_str_radix(size_line.split(';').next().unwrap_or_default().trim(), 16) else {
            return false;
        };
        cursor = line_end + 2;
        if size == 0 {
            return body.get(cursor..cursor + 2) == Some(b"\r\n");
        }
        if body.len() < cursor + size + 2 {
            return false;
        }
        cursor += size;
        if body.get(cursor..cursor + 2) != Some(b"\r\n") {
            return false;
        }
        cursor += 2;
    }
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
            protocol: crate::RouteProtocol::All,
        },
    );
    resources.domain_rules.insert(
        tunnel_host.to_string(),
        DomainRule {
            port_min: 443,
            port_max: 443,
            protocol: crate::RouteProtocol::All,
        },
    );
    resources.ip_rules.push(IpRule {
        ip_min: resolved_ip,
        ip_max: resolved_ip,
        port_min: 1,
        port_max: 65535,
        protocol: crate::RouteProtocol::All,
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
