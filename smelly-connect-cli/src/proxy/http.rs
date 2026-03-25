use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::pool::SessionPool;

#[derive(Debug, Clone)]
pub struct HttpProxyTestResult {
    pub body: String,
    pub account_name: String,
    pub used_pool_selection: bool,
}

#[derive(Debug, Clone)]
pub struct ConnectProxyTestResult {
    pub account_name: String,
    pub echoed_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct NoReadySessionResult {
    pub status_code: u16,
}

pub async fn proxy_http_for_test() -> Result<HttpProxyTestResult, String> {
    let upstream = spawn_http_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let selected = Arc::new(Mutex::new(None::<String>));
    let addr = spawn_test_proxy(pool, {
        let selected = Arc::clone(&selected);
        move |account_name, _host, _port| {
            let selected = Arc::clone(&selected);
            async move {
                *selected.lock().await = Some(account_name);
                TcpStream::connect(upstream).await
            }
        }
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"GET http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let account_name = selected
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no account selected".to_string())?;
    Ok(HttpProxyTestResult {
        body,
        account_name,
        used_pool_selection: true,
    })
}

pub async fn proxy_connect_for_test() -> Result<ConnectProxyTestResult, String> {
    let upstream = spawn_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let selected = Arc::new(Mutex::new(None::<String>));
    let addr = spawn_test_proxy(pool, {
        let selected = Arc::clone(&selected);
        move |account_name, _host, _port| {
            let selected = Arc::clone(&selected);
            async move {
                *selected.lock().await = Some(account_name);
                TcpStream::connect(upstream).await
            }
        }
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"CONNECT libdb.zju.edu.cn:443 HTTP/1.1\r\nHost: libdb.zju.edu.cn:443\r\nConnection: close\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;

    let mut header = [0_u8; 128];
    let n = client
        .read(&mut header)
        .await
        .map_err(|err| err.to_string())?;
    let header = String::from_utf8_lossy(&header[..n]);
    if !header.starts_with("HTTP/1.1 200") {
        return Err(format!("unexpected connect response: {header}"));
    }

    client
        .write_all(b"ping")
        .await
        .map_err(|err| err.to_string())?;
    let mut echoed = [0_u8; 4];
    client
        .read_exact(&mut echoed)
        .await
        .map_err(|err| err.to_string())?;
    let account_name = selected
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no account selected".to_string())?;
    Ok(ConnectProxyTestResult {
        account_name,
        echoed_bytes: echoed.to_vec(),
    })
}

pub async fn proxy_http_no_ready_session_for_test() -> Result<NoReadySessionResult, String> {
    let pool = SessionPool::from_failed_accounts(1).await;
    let addr = spawn_test_proxy(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    request_no_ready_session(addr).await
}

pub async fn proxy_http_no_ready_session_sequence_for_test(
    count: usize,
) -> Result<Vec<NoReadySessionResult>, String> {
    let pool = SessionPool::from_failed_accounts(1).await;
    let addr = spawn_test_proxy(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        results.push(request_no_ready_session(addr).await?);
    }
    Ok(results)
}

pub async fn serve_http(listen: String, pool: SessionPool) -> Result<(), String> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|err| err.to_string())?;
    let local_addr = listener.local_addr().map_err(|err| err.to_string())?;
    tracing::info!(
        protocol = tracing::field::display("http"),
        listen = %local_addr,
        "http proxy listening"
    );
    loop {
        let (stream, _) = listener.accept().await.map_err(|err| err.to_string())?;
        let pool = pool.clone();
        tokio::spawn(async move {
            let _ = handle_live_client(stream, pool).await;
        });
    }
}

struct ForwardRequest<'a> {
    method: &'a str,
    target: &'a str,
    version: &'a str,
    headers: Vec<&'a str>,
    leftover: Vec<u8>,
}

async fn spawn_test_proxy<F, Fut>(pool: SessionPool, connector: F) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let pool = pool.clone();
            let connector = connector.clone();
            tokio::spawn(async move {
                let _ = handle_client(stream, pool, connector).await;
            });
        }
    });
    Ok(addr)
}

async fn request_no_ready_session(addr: SocketAddr) -> Result<NoReadySessionResult, String> {
    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"GET http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let status_line = response.lines().next().unwrap_or_default().to_string();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("invalid status line: {status_line}"))?;
    Ok(NoReadySessionResult { status_code })
}

async fn handle_client<F, Fut>(
    mut client: TcpStream,
    pool: SessionPool,
    connector: F,
) -> Result<(), String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let mut buffer = Vec::with_capacity(1024);
    let header_end = read_headers(&mut client, &mut buffer)
        .await
        .map_err(|err| err.to_string())?;
    let header_bytes = &buffer[..header_end];
    let leftover = buffer[header_end..].to_vec();
    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");

    let account_name = match pool.next_account_name().await {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                "no ready session"
            );
            client
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
    };

    if method.eq_ignore_ascii_case("CONNECT") {
        tracing::info!(
            protocol = tracing::field::display("connect"),
            target = %target,
            account = %account_name,
            "request accepted"
        );
        return handle_connect(account_name, connector, client, target, leftover).await;
    }

    tracing::info!(
        protocol = tracing::field::display("http"),
        target = %target,
        account = %account_name,
        "request accepted"
    );

    handle_forward(
        account_name,
        connector,
        client,
        ForwardRequest {
            method,
            target,
            version,
            headers: lines.collect(),
            leftover,
        },
    )
    .await
}

async fn handle_live_client(mut client: TcpStream, pool: SessionPool) -> Result<(), String> {
    let mut buffer = Vec::with_capacity(1024);
    let header_end = read_headers(&mut client, &mut buffer)
        .await
        .map_err(|err| err.to_string())?;
    let header_bytes = &buffer[..header_end];
    let leftover = buffer[header_end..].to_vec();
    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");

    let (account_name, session) = match pool.next_live_session().await {
        Ok(ready) => ready,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                "no ready session"
            );
            client
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
    };

    if method.eq_ignore_ascii_case("CONNECT") {
        tracing::info!(
            protocol = tracing::field::display("connect"),
            target = %target,
            account = %account_name,
            "request accepted"
        );
        let (host, port) = split_host_port(target, 443)?;
        let mut upstream = session
            .connect_tcp((host, port))
            .await
            .map_err(|err| format!("{account_name}: {err:?}"))?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .map_err(|err| err.to_string())?;
        if !leftover.is_empty() {
            upstream
                .write_all(&leftover)
                .await
                .map_err(|err| err.to_string())?;
        }
        let _ = copy_bidirectional(&mut client, &mut upstream)
            .await
            .map_err(|err| err.to_string())?;
        return Ok(());
    }

    tracing::info!(
        protocol = tracing::field::display("http"),
        target = %target,
        account = %account_name,
        "request accepted"
    );

    let (host, port, path) = parse_absolute_target(target)?;
    let mut upstream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(|err| format!("{account_name}: {err:?}"))?;

    let mut upstream_request = format!("{method} {path} {version}\r\n");
    for header in lines {
        if header.to_ascii_lowercase().starts_with("proxy-connection:") {
            continue;
        }
        upstream_request.push_str(header);
        upstream_request.push_str("\r\n");
    }
    upstream_request.push_str("\r\n");

    upstream
        .write_all(upstream_request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    if !leftover.is_empty() {
        upstream
            .write_all(&leftover)
            .await
            .map_err(|err| err.to_string())?;
    }
    tokio::io::copy(&mut upstream, &mut client)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn handle_connect<F, Fut>(
    account_name: String,
    connector: F,
    mut client: TcpStream,
    target: &str,
    leftover: Vec<u8>,
) -> Result<(), String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let (host, port) = split_host_port(target, 443)?;
    let mut upstream = connector(account_name, host.to_string(), port)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .map_err(|err| err.to_string())?;
    if !leftover.is_empty() {
        upstream
            .write_all(&leftover)
            .await
            .map_err(|err| err.to_string())?;
    }
    let _ = copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn handle_forward<F, Fut>(
    account_name: String,
    connector: F,
    mut client: TcpStream,
    request: ForwardRequest<'_>,
) -> Result<(), String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let (host, port, path) = parse_absolute_target(request.target)?;
    let mut upstream = connector(account_name, host.clone(), port)
        .await
        .map_err(|err| err.to_string())?;

    let mut upstream_request = format!("{} {path} {}\r\n", request.method, request.version);
    for header in request.headers {
        if header.to_ascii_lowercase().starts_with("proxy-connection:") {
            continue;
        }
        upstream_request.push_str(header);
        upstream_request.push_str("\r\n");
    }
    upstream_request.push_str("\r\n");

    upstream
        .write_all(upstream_request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    if !request.leftover.is_empty() {
        upstream
            .write_all(&request.leftover)
            .await
            .map_err(|err| err.to_string())?;
    }
    tokio::io::copy(&mut upstream, &mut client)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn read_headers(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> io::Result<usize> {
    let mut chunk = [0_u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed",
            ));
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(index) = find_header_end(buffer) {
            return Ok(index);
        }
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)
}

fn parse_absolute_target(target: &str) -> Result<(String, u16, String), String> {
    let without_scheme = target
        .strip_prefix("http://")
        .ok_or_else(|| "unsupported scheme".to_string())?;
    let mut parts = without_scheme.splitn(2, '/');
    let authority = parts.next().unwrap_or_default();
    let path = format!("/{}", parts.next().unwrap_or_default());
    let (host, port) = split_host_port(authority, 80)?;
    Ok((host.to_string(), port, path))
}

fn split_host_port(target: &str, default_port: u16) -> Result<(&str, u16), String> {
    if let Some((host, port)) = target.rsplit_once(':') {
        let port = port.parse().map_err(|_| "invalid port".to_string())?;
        Ok((host, port))
    } else {
        Ok((target, default_port))
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
