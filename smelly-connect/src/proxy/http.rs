use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::session::EasyConnectSession;

#[derive(Debug, Clone, Copy)]
enum RequestBodyKind {
    None,
    ContentLength(usize),
    Chunked,
}

pub struct ProxyHandle {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

pub type HttpProxyHandle = ProxyHandle;

impl ProxyHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(mut self) -> io::Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

pub async fn start_http_proxy(
    session: EasyConnectSession,
    bind: SocketAddr,
) -> io::Result<ProxyHandle> {
    let listener = TcpListener::bind(bind).await?;
    let local_addr = listener.local_addr()?;
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let session = session.clone();
                    tokio::spawn(async move {
                        let _ = handle_client(session, stream).await;
                    });
                }
            }
        }
    });

    Ok(ProxyHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
    })
}

async fn handle_client(session: EasyConnectSession, mut client: TcpStream) -> io::Result<()> {
    let mut buffer = Vec::with_capacity(1024);
    let header_end = read_headers(&mut client, &mut buffer).await?;
    let header_bytes = &buffer[..header_end];
    let leftover = buffer[header_end..].to_vec();
    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");
    let headers: Vec<&str> = lines.collect();
    let body_kind = parse_request_body_kind(&headers);

    if method.eq_ignore_ascii_case("CONNECT") {
        return handle_connect(session, client, target, leftover).await;
    }

    handle_forward(
        session,
        client,
        method,
        target,
        version,
        headers,
        leftover,
        body_kind,
    )
    .await
}

async fn handle_connect(
    session: EasyConnectSession,
    mut client: TcpStream,
    target: &str,
    leftover: Vec<u8>,
) -> io::Result<()> {
    let (host, port) = split_host_port(target, 443)?;
    let mut upstream = session.connect_tcp((host, port)).await.map_err(other_io)?;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    if !leftover.is_empty() {
        upstream.write_all(&leftover).await?;
    }
    let _ = copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

async fn handle_forward(
    session: EasyConnectSession,
    mut client: TcpStream,
    method: &str,
    target: &str,
    version: &str,
    headers: Vec<&str>,
    leftover: Vec<u8>,
    body_kind: RequestBodyKind,
) -> io::Result<()> {
    let (host, port, path) = parse_absolute_target(target)?;
    let mut upstream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(other_io)?;

    let mut request = format!("{method} {path} {version}\r\n");
    for header in headers {
        let lower = header.to_ascii_lowercase();
        if lower.starts_with("proxy-connection:")
            || lower.starts_with("connection:")
            || lower.starts_with("keep-alive:")
        {
            continue;
        }
        request.push_str(header);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n");
    request.push_str("\r\n");

    upstream.write_all(request.as_bytes()).await?;
    if !leftover.is_empty() {
        upstream.write_all(&leftover).await?;
    }
    stream_remaining_request_body(&mut client, &mut upstream, &leftover, body_kind).await?;
    tokio::io::copy(&mut upstream, &mut client).await?;
    Ok(())
}

async fn stream_remaining_request_body(
    client: &mut TcpStream,
    upstream: &mut crate::transport::VpnStream,
    leftover: &[u8],
    body_kind: RequestBodyKind,
) -> io::Result<()> {
    match body_kind {
        RequestBodyKind::None => Ok(()),
        RequestBodyKind::ContentLength(content_length) => {
            if leftover.len() >= content_length {
                return Ok(());
            }

            let mut remaining = content_length - leftover.len();
            let mut chunk = [0_u8; 8192];
            while remaining > 0 {
                let limit = remaining.min(chunk.len());
                let n = client.read(&mut chunk[..limit]).await?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed before request body completed",
                    ));
                }
                upstream.write_all(&chunk[..n]).await?;
                remaining -= n;
            }
            Ok(())
        }
        RequestBodyKind::Chunked => {
            let mut tracker = ChunkedBodyTracker::new();
            if tracker.feed(leftover)? {
                return Ok(());
            }

            let mut chunk = [0_u8; 8192];
            loop {
                let n = client.read(&mut chunk).await?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed before chunked request body completed",
                    ));
                }
                upstream.write_all(&chunk[..n]).await?;
                if tracker.feed(&chunk[..n])? {
                    return Ok(());
                }
            }
        }
    }
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

fn parse_content_length(headers: &[&str]) -> Option<usize> {
    headers.iter().find_map(|header| {
        header.split_once(':').and_then(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
    })
}

fn has_chunked_transfer_encoding(headers: &[&str]) -> bool {
    headers.iter().any(|header| {
        header.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        })
    })
}

fn parse_request_body_kind(headers: &[&str]) -> RequestBodyKind {
    if has_chunked_transfer_encoding(headers) {
        RequestBodyKind::Chunked
    } else if let Some(content_length) = parse_content_length(headers) {
        RequestBodyKind::ContentLength(content_length)
    } else {
        RequestBodyKind::None
    }
}

struct ChunkedBodyTracker {
    state: ChunkedState,
}

enum ChunkedState {
    SizeLine(Vec<u8>),
    Data(usize),
    DataCrLf(usize),
    Trailers(Vec<u8>),
    Done,
}

impl ChunkedBodyTracker {
    fn new() -> Self {
        Self {
            state: ChunkedState::SizeLine(Vec::new()),
        }
    }

    fn feed(&mut self, input: &[u8]) -> io::Result<bool> {
        let mut idx = 0usize;
        while idx < input.len() {
            match &mut self.state {
                ChunkedState::SizeLine(buffer) => {
                    buffer.push(input[idx]);
                    idx += 1;
                    if buffer.ends_with(b"\r\n") {
                        let line = std::str::from_utf8(&buffer[..buffer.len() - 2]).map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size line")
                        })?;
                        let size_text = line.split(';').next().unwrap_or_default().trim();
                        let size = usize::from_str_radix(size_text, 16).map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size")
                        })?;
                        self.state = if size == 0 {
                            ChunkedState::Trailers(Vec::new())
                        } else {
                            ChunkedState::Data(size)
                        };
                    }
                }
                ChunkedState::Data(remaining) => {
                    let take = (*remaining).min(input.len() - idx);
                    *remaining -= take;
                    idx += take;
                    if *remaining == 0 {
                        self.state = ChunkedState::DataCrLf(0);
                    }
                }
                ChunkedState::DataCrLf(seen) => {
                    let expected = if *seen == 0 { b'\r' } else { b'\n' };
                    if input[idx] != expected {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid chunk delimiter",
                        ));
                    }
                    *seen += 1;
                    idx += 1;
                    if *seen == 2 {
                        self.state = ChunkedState::SizeLine(Vec::new());
                    }
                }
                ChunkedState::Trailers(buffer) => {
                    buffer.push(input[idx]);
                    idx += 1;
                    if buffer == b"\r\n" || buffer.ends_with(b"\r\n\r\n") {
                        self.state = ChunkedState::Done;
                        return Ok(true);
                    }
                }
                ChunkedState::Done => return Ok(true),
            }
        }
        Ok(matches!(self.state, ChunkedState::Done))
    }
}

fn parse_absolute_target(target: &str) -> io::Result<(String, u16, String)> {
    let without_scheme = target
        .strip_prefix("http://")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsupported scheme"))?;
    let mut parts = without_scheme.splitn(2, '/');
    let authority = parts.next().unwrap_or_default();
    let path = format!("/{}", parts.next().unwrap_or_default());
    let (host, port) = split_host_port(authority, 80)?;
    Ok((host.to_string(), port, path))
}

fn split_host_port(target: &str, default_port: u16) -> io::Result<(&str, u16)> {
    if let Some((host, port)) = target.rsplit_once(':') {
        let port = port
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid port"))?;
        Ok((host, port))
    } else {
        Ok((target, default_port))
    }
}

fn other_io(err: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{err:?}"))
}
