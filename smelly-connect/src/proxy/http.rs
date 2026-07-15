use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use crate::session::EasyConnectSession;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const HEADER_TOO_LARGE_MESSAGE: &str = "request header too large";

#[derive(Debug, Clone, Copy)]
enum RequestBodyKind {
    None,
    ContentLength(usize),
    Chunked,
}

#[derive(Debug, Clone, Copy)]
enum RequestControl {
    Standard,
    ExpectContinue,
}

struct ForwardRequest<'a> {
    method: &'a str,
    target: &'a str,
    version: &'a str,
    headers: Vec<&'a [u8]>,
    leftover: Vec<u8>,
    body_kind: RequestBodyKind,
    request_control: RequestControl,
}

pub struct ProxyHandle {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(mut self) -> io::Result<()> {
        self.signal_shutdown();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        Ok(())
    }

    fn signal_shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.signal_shutdown();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn start_http_proxy(
    session: EasyConnectSession,
    bind: SocketAddr,
) -> io::Result<ProxyHandle> {
    let listener = TcpListener::bind(bind).await?;
    let local_addr = listener.local_addr()?;
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    let task = tokio::spawn(async move {
        let mut client_tasks = JoinSet::new();
        let mut abort_clients = false;
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    abort_clients = true;
                    break;
                }
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let session = session.clone();
                    client_tasks.spawn(async move {
                        let _ = handle_client(session, stream).await;
                    });
                }
                joined = client_tasks.join_next(), if !client_tasks.is_empty() => {
                    let _ = joined;
                }
            }
        }

        if abort_clients {
            client_tasks.abort_all();
        }
        while let Some(joined) = client_tasks.join_next().await {
            let _ = joined;
        }
    });

    Ok(ProxyHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        task: Some(task),
    })
}

async fn handle_client(session: EasyConnectSession, mut client: TcpStream) -> io::Result<()> {
    let mut buffer = Vec::with_capacity(1024);
    let header_end = match read_headers(&mut client, &mut buffer).await {
        Ok(header_end) => header_end,
        Err(err) if is_header_too_large(&err) => {
            client
                .write_all(
                    b"HTTP/1.1 431 Request Header Fields Too Large\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await?;
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    let header_bytes = &buffer[..header_end];
    let leftover = buffer[header_end..].to_vec();
    let mut lines = header_lines(header_bytes);
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))
        .and_then(|line| {
            std::str::from_utf8(line)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid request line"))
        })?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");
    let headers: Vec<&[u8]> = lines.collect();
    let body_kind = parse_request_body_kind(&headers);
    let request_control = parse_request_control(&headers);

    if method.eq_ignore_ascii_case("CONNECT") {
        return handle_connect(session, client, target, leftover).await;
    }

    handle_forward(
        session,
        client,
        ForwardRequest {
            method,
            target,
            version,
            headers,
            leftover,
            body_kind,
            request_control,
        },
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
    request: ForwardRequest<'_>,
) -> io::Result<()> {
    let ForwardRequest {
        method,
        target,
        version,
        headers,
        leftover,
        body_kind,
        request_control,
    } = request;
    let (host, port, path) = parse_absolute_target(target)?;
    let mut upstream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(other_io)?;

    let mut request = format!("{method} {path} {version}\r\n").into_bytes();
    for header in headers {
        if should_strip_request_header(header) {
            continue;
        }
        request.extend_from_slice(header);
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"Connection: close\r\n\r\n");

    upstream.write_all(&request).await?;
    if !leftover.is_empty() {
        upstream.write_all(&leftover).await?;
    }
    if matches!(request_control, RequestControl::ExpectContinue) {
        client.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
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
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                HEADER_TOO_LARGE_MESSAGE,
            ));
        }
        if let Some(index) = find_header_end(buffer) {
            return Ok(index);
        }
    }
}

fn is_header_too_large(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::InvalidData && err.to_string() == HEADER_TOO_LARGE_MESSAGE
}

pub fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)
}

fn header_lines(buffer: &[u8]) -> impl Iterator<Item = &[u8]> {
    buffer
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty())
}

fn split_header_bytes(header: &[u8]) -> Option<(&[u8], &[u8])> {
    let separator = header.iter().position(|byte| *byte == b':')?;
    Some((
        trim_ascii_http_whitespace(&header[..separator]),
        trim_ascii_http_whitespace(&header[separator + 1..]),
    ))
}

fn trim_ascii_http_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|idx| idx + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn should_strip_request_header(header: &[u8]) -> bool {
    split_header_bytes(header).is_some_and(|(name, _)| {
        name.eq_ignore_ascii_case(b"proxy-connection")
            || name.eq_ignore_ascii_case(b"proxy-authorization")
            || name.eq_ignore_ascii_case(b"connection")
            || name.eq_ignore_ascii_case(b"keep-alive")
            || name.eq_ignore_ascii_case(b"expect")
    })
}

fn parse_content_length_bytes(headers: &[&[u8]]) -> Option<usize> {
    headers.iter().find_map(|header| {
        split_header_bytes(header).and_then(|(name, value)| {
            name.eq_ignore_ascii_case(b"content-length")
                .then(|| {
                    std::str::from_utf8(value)
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .flatten()
        })
    })
}

pub fn parse_content_length(headers: &[&str]) -> Option<usize> {
    let headers: Vec<&[u8]> = headers.iter().map(|header| header.as_bytes()).collect();
    parse_content_length_bytes(&headers)
}

fn has_chunked_transfer_encoding_bytes(headers: &[&[u8]]) -> bool {
    headers.iter().any(|header| {
        split_header_bytes(header).is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case(b"transfer-encoding")
                && std::str::from_utf8(value).is_ok_and(|value| {
                    value
                        .split(',')
                        .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
                })
        })
    })
}

pub fn has_chunked_transfer_encoding(headers: &[&str]) -> bool {
    let headers: Vec<&[u8]> = headers.iter().map(|header| header.as_bytes()).collect();
    has_chunked_transfer_encoding_bytes(&headers)
}

fn parse_request_body_kind(headers: &[&[u8]]) -> RequestBodyKind {
    if has_chunked_transfer_encoding_bytes(headers) {
        RequestBodyKind::Chunked
    } else if let Some(content_length) = parse_content_length_bytes(headers) {
        RequestBodyKind::ContentLength(content_length)
    } else {
        RequestBodyKind::None
    }
}

fn parse_request_control(headers: &[&[u8]]) -> RequestControl {
    if headers.iter().any(|header| {
        split_header_bytes(header).is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case(b"expect")
                && std::str::from_utf8(value).is_ok_and(|value| {
                    value
                        .split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("100-continue"))
                })
        })
    }) {
        RequestControl::ExpectContinue
    } else {
        RequestControl::Standard
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
                        let line =
                            std::str::from_utf8(&buffer[..buffer.len() - 2]).map_err(|_| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "invalid chunk size line",
                                )
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
