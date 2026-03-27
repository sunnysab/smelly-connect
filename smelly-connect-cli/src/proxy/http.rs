use std::convert::Infallible;
#[cfg(any(test, debug_assertions))]
use std::future::Future;
use std::io;
#[cfg(any(test, debug_assertions))]
use std::net::SocketAddr;
use std::pin::Pin;
#[cfg(any(test, debug_assertions))]
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{CONNECTION, EXPECT, HOST, PROXY_AUTHORIZATION};
use http::{HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::{Body as HyperBody, Frame, Incoming};
use hyper::server::conn::http1 as hyper_server_http1;
use hyper::service::service_fn;
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
#[cfg(any(test, debug_assertions))]
use tokio::sync::Mutex;
use tokio::sync::mpsc;
#[cfg(any(test, debug_assertions))]
use tokio::time::Instant;

use crate::pool::SessionPool;
#[cfg(any(test, debug_assertions))]
use crate::runtime::RuntimeSnapshot;
use crate::runtime::{ConnectionGuard, ProxyProtocol, RuntimeStats};

type ProxyBody = BoxBody<Bytes, io::Error>;

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct HttpProxyTestResult {
    pub body: String,
    pub account_name: String,
    pub used_pool_selection: bool,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct HttpBodyTestResult {
    pub body: String,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct StreamingResponseTestResult {
    pub first_chunk_latency: Duration,
    pub first_chunk: String,
    pub full_body: String,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct ConnectProxyTestResult {
    pub account_name: String,
    pub echoed_bytes: Vec<u8>,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct NoReadySessionResult {
    pub status_code: u16,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct TimeoutTestResult {
    pub elapsed: Duration,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct LiveFailureRecoveryTestResult {
    pub status_code: u16,
    pub state_summary: String,
    pub selectable_after_failure: bool,
    pub recovered_account: String,
}

#[cfg(any(test, debug_assertions))]
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
enum UpstreamConnectError {
    TimedOut,
    Failed(String),
    RouteRejected,
}

enum ResponseBodyKind {
    None,
    ContentLength(usize),
    Chunked,
    ReadToEnd,
}

struct CountedBody<B> {
    inner: B,
    connection: Option<ConnectionGuard>,
}

impl<B> CountedBody<B> {
    fn new(inner: B, connection: Option<ConnectionGuard>) -> Self {
        Self { inner, connection }
    }
}

impl<B> HyperBody for CountedBody<B>
where
    B: HyperBody<Data = Bytes, Error = io::Error> + Unpin,
{
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref()
                    && let Some(connection) = &self.connection
                {
                    connection.add_upstream_to_client_bytes(data.len() as u64);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }
}

struct ChannelBody {
    rx: mpsc::Receiver<Result<Bytes, io::Error>>,
}

impl ChannelBody {
    fn new(rx: mpsc::Receiver<Result<Bytes, io::Error>>) -> Self {
        Self { rx }
    }
}

impl HyperBody for ChannelBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(Ok(chunk))) => Poll::Ready(Some(Ok(Frame::data(chunk)))),
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

struct ChunkedResponseDecoder {
    state: ChunkedState,
}

impl ChunkedResponseDecoder {
    fn new() -> Self {
        Self {
            state: ChunkedState::SizeLine(Vec::new()),
        }
    }

    fn feed(&mut self, input: &[u8]) -> io::Result<Vec<Bytes>> {
        let mut idx = 0usize;
        let mut decoded = Vec::new();
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
                    if take > 0 {
                        decoded.push(Bytes::copy_from_slice(&input[idx..idx + take]));
                        *remaining -= take;
                        idx += take;
                    }
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
                    }
                }
                ChunkedState::Done => break,
            }
        }
        Ok(decoded)
    }

    fn is_done(&self) -> bool {
        matches!(self.state, ChunkedState::Done)
    }
}

impl UpstreamConnectError {
    fn label(&self) -> String {
        match self {
            Self::TimedOut => "connect timed out".to_string(),
            Self::Failed(message) => message.clone(),
            Self::RouteRejected => "route rejected".to_string(),
        }
    }
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_origin_form_for_test() -> Result<HttpBodyTestResult, String> {
    let upstream = spawn_http_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"GET /health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
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
    Ok(HttpBodyTestResult { body })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_body_completes_for_keep_alive_upstream_for_test(
) -> Result<HttpBodyTestResult, String> {
    let upstream = spawn_keep_alive_http_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"GET http://intranet.zju.edu.cn/index.html HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: keep-alive\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<Vec<u8>, String>(response)
    })
    .await
    .map_err(|_| "proxy response timed out".to_string())??;

    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    Ok(HttpBodyTestResult { body })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_streams_request_body_for_test() -> Result<HttpBodyTestResult, String> {
    let upstream = spawn_request_body_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"POST http://intranet.zju.edu.cn/upload HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello",
        )
        .await
        .map_err(|err| err.to_string())?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client
        .write_all(b" world")
        .await
        .map_err(|err| err.to_string())?;
    client.shutdown().await.map_err(|err| err.to_string())?;

    let response = tokio::time::timeout(Duration::from_secs(1), async {
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<Vec<u8>, String>(response)
    })
    .await
    .map_err(|_| "proxy response timed out".to_string())??;

    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    Ok(HttpBodyTestResult { body })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_streams_chunked_request_body_for_test(
) -> Result<HttpBodyTestResult, String> {
    let upstream = spawn_chunked_request_body_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"POST http://intranet.zju.edu.cn/upload HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client
        .write_all(b"6\r\n world\r\n0\r\n\r\n")
        .await
        .map_err(|err| err.to_string())?;
    client.shutdown().await.map_err(|err| err.to_string())?;

    let response = tokio::time::timeout(Duration::from_secs(1), async {
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<Vec<u8>, String>(response)
    })
    .await
    .map_err(|_| "proxy response timed out".to_string())??;

    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    Ok(HttpBodyTestResult { body })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_expect_continue_for_test() -> Result<HttpBodyTestResult, String> {
    let upstream = spawn_request_body_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"POST http://intranet.zju.edu.cn/upload HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nContent-Length: 11\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;

    let interim = tokio::time::timeout(Duration::from_millis(300), async {
        let mut buf = [0_u8; 128];
        let n = client.read(&mut buf).await.map_err(|err| err.to_string())?;
        Ok::<String, String>(String::from_utf8_lossy(&buf[..n]).to_string())
    })
    .await
    .map_err(|_| "proxy did not send 100 Continue".to_string())??;
    if !interim.starts_with("HTTP/1.1 100 Continue") {
        return Err(format!("unexpected interim response: {interim}"));
    }

    client
        .write_all(b"hello world")
        .await
        .map_err(|err| err.to_string())?;
    client.shutdown().await.map_err(|err| err.to_string())?;

    let response = tokio::time::timeout(Duration::from_secs(1), async {
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<Vec<u8>, String>(response)
    })
    .await
    .map_err(|_| "proxy response timed out".to_string())??;

    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    Ok(HttpBodyTestResult { body })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_strips_proxy_authorization_for_test(
) -> Result<HttpBodyTestResult, String> {
    let upstream = spawn_proxy_auth_capture_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"GET http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nProxy-Authorization: Basic Zm9vOmJhcg==\r\nConnection: close\r\n\r\n",
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
    Ok(HttpBodyTestResult { body })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_streams_response_body_for_test(
) -> Result<StreamingResponseTestResult, String> {
    let upstream = spawn_slow_streaming_response_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy(pool, move |_account_name, _host, _port| async move {
        TcpStream::connect(upstream).await
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"GET http://intranet.zju.edu.cn/stream HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;

    let started = Instant::now();
    let first_chunk = tokio::time::timeout(Duration::from_millis(150), async {
        let mut response = vec![0_u8; 128];
        let n = client
            .read(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<Vec<u8>, String>(response[..n].to_vec())
    })
    .await
    .map_err(|_| "proxy did not stream first response chunk in time".to_string())??;
    let first_chunk_latency = started.elapsed();

    let mut response = first_chunk;
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let full_body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let first_chunk = full_body.chars().take(5).collect();

    Ok(StreamingResponseTestResult {
        first_chunk_latency,
        first_chunk,
        full_body,
    })
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_no_ready_session_for_test() -> Result<NoReadySessionResult, String> {
    let pool = SessionPool::from_failed_accounts(1).await;
    let addr = spawn_test_proxy(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    request_no_ready_session(addr).await
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_runtime_stats_for_test() -> Result<RuntimeSnapshot, String> {
    let upstream = spawn_http_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let stats = RuntimeStats::default();
    let addr = spawn_test_proxy_with_stats(
        pool.clone(),
        stats.clone(),
        move |_account_name, _host, _port| async move { TcpStream::connect(upstream).await },
    )
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

    Ok(stats.snapshot(pool.summary().await))
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_connect_timeout_for_test() -> Result<TimeoutTestResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy_with_timeout(
        pool,
        Duration::from_millis(20),
        |_account_name, _host, _port| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Err(io::Error::new(io::ErrorKind::TimedOut, "slow upstream"))
        },
    )
    .await?;

    let started = Instant::now();
    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"CONNECT libdb.zju.edu.cn:443 HTTP/1.1\r\nHost: libdb.zju.edu.cn:443\r\nConnection: close\r\n\r\n",
        )
        .await
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    Ok(TimeoutTestResult {
        elapsed: started.elapsed(),
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_connect_failure_status_for_test() -> Result<NoReadySessionResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy_with_timeout(
        pool,
        Duration::from_millis(20),
        |_account_name, _host, _port| async move { Err(io::Error::other("upstream failed")) },
    )
    .await?;
    request_connect_status(addr).await
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_connect_timeout_status_for_test() -> Result<NoReadySessionResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_proxy_with_timeout(
        pool,
        Duration::from_millis(20),
        |_account_name, _host, _port| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Err(io::Error::new(io::ErrorKind::TimedOut, "slow upstream"))
        },
    )
    .await?;
    request_connect_status(addr).await
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_live_connect_failure_recovery_for_test(
) -> Result<LiveFailureRecoveryTestResult, String> {
    let session = smelly_connect::session::tests::session_with_failing_domain_match(
        "libdb.zju.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let serve_pool = pool.clone();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let _ = handle_live_client(stream, serve_pool, RuntimeStats::default(), DEFAULT_CONNECT_TIMEOUT).await;
    });

    let status = request_connect_status(addr).await?;
    let state_summary = pool.state_summary_for_test().await;
    let selectable_after_failure = pool.has_selectable_nodes_for_test().await;
    tokio::time::advance(Duration::from_secs(61)).await;
    let recovered = pool.try_request_triggered_probe_for_test().await.unwrap();
    Ok(LiveFailureRecoveryTestResult {
        status_code: status.status_code,
        state_summary,
        selectable_after_failure,
        recovered_account: recovered.account_name().to_string(),
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_route_rejection_does_not_open_for_test(
) -> Result<LiveFailureRecoveryTestResult, String> {
    let session = smelly_connect::session::tests::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let serve_pool = pool.clone();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let _ = handle_live_client(
            stream,
            serve_pool,
            RuntimeStats::default(),
            DEFAULT_CONNECT_TIMEOUT,
        )
        .await;
    });

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"CONNECT xg.sit.edu.cn:443 HTTP/1.1\r\nHost: xg.sit.edu.cn:443\r\nConnection: close\r\n\r\n",
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
    Ok(LiveFailureRecoveryTestResult {
        status_code,
        state_summary: pool.state_summary_for_test().await,
        selectable_after_failure: pool.has_selectable_nodes_for_test().await,
        recovered_account: "acct-01".to_string(),
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_timeout_does_not_open_for_test(
) -> Result<LiveFailureRecoveryTestResult, String> {
    let session = smelly_connect::session::tests::session_with_slow_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let serve_pool = pool.clone();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let _ = handle_live_client(
            stream,
            serve_pool,
            RuntimeStats::default(),
            Duration::from_millis(20),
        )
        .await;
    });

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"CONNECT jwxt.sit.edu.cn:443 HTTP/1.1\r\nHost: jwxt.sit.edu.cn:443\r\nConnection: close\r\n\r\n",
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
    Ok(LiveFailureRecoveryTestResult {
        status_code,
        state_summary: pool.state_summary_for_test().await,
        selectable_after_failure: pool.has_selectable_nodes_for_test().await,
        recovered_account: "acct-01".to_string(),
    })
}

pub async fn serve_http(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
) -> Result<(), String> {
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
        let stats = stats.clone();
        let connect_timeout = connect_timeout;
        tokio::spawn(async move {
            if let Err(err) = handle_live_client(stream, pool, stats, connect_timeout).await {
                tracing::warn!(
                    protocol = tracing::field::display("http"),
                    error = %err,
                    "live proxy request failed"
                );
            }
        });
    }
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_http_live_failure_for_test() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let pool = SessionPool::from_failed_accounts(1).await;
    let stats = RuntimeStats::default();

    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        if let Err(err) = handle_live_client(stream, pool, stats, DEFAULT_CONNECT_TIMEOUT).await {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                error = %err,
                "live proxy request failed"
            );
        } else {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                error = "connection closed before request completed",
                "live proxy request failed"
            );
        }
    });

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(b"\r\n\r\n")
        .await
        .map_err(|err| err.to_string())?;
    let _ = client.shutdown().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    Ok(())
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_proxy<F, Fut>(pool: SessionPool, connector: F) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_proxy_internal(pool, None, DEFAULT_CONNECT_TIMEOUT, connector).await
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_proxy_with_stats<F, Fut>(
    pool: SessionPool,
    stats: RuntimeStats,
    connector: F,
) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_proxy_internal(pool, Some(stats), DEFAULT_CONNECT_TIMEOUT, connector).await
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_proxy_with_timeout<F, Fut>(
    pool: SessionPool,
    connect_timeout: Duration,
    connector: F,
) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_proxy_internal(pool, None, connect_timeout, connector).await
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_proxy_internal<F, Fut>(
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connect_timeout: Duration,
    connector: F,
) -> Result<SocketAddr, String>
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
            let stats = stats.clone();
            let connector = connector.clone();
            let connect_timeout = connect_timeout;
            tokio::spawn(async move {
                let _ = handle_client(stream, pool, stats, connect_timeout, connector).await;
            });
        }
    });
    Ok(addr)
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
async fn request_connect_status(addr: SocketAddr) -> Result<NoReadySessionResult, String> {
    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(
            b"CONNECT libdb.zju.edu.cn:443 HTTP/1.1\r\nHost: libdb.zju.edu.cn:443\r\nConnection: close\r\n\r\n",
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

#[cfg(any(test, debug_assertions))]
async fn handle_client<F, Fut>(
    client: TcpStream,
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connect_timeout: Duration,
    connector: F,
) -> Result<(), String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let io = TokioIo::new(client);
    hyper_server_http1::Builder::new()
        .half_close(true)
        .serve_connection(
            io,
            service_fn(move |request| {
                let pool = pool.clone();
                let stats = stats.clone();
                let connector = connector.clone();
                async move {
                    Ok::<_, Infallible>(
                        handle_test_request(request, pool, stats, connect_timeout, connector).await,
                    )
                }
            }),
        )
        .with_upgrades()
        .await
        .map_err(|err| err.to_string())
}

async fn handle_live_client(
    client: TcpStream,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
) -> Result<(), String> {
    let io = TokioIo::new(client);
    hyper_server_http1::Builder::new()
        .half_close(true)
        .serve_connection(
            io,
            service_fn(move |request| {
                let pool = pool.clone();
                let stats = stats.clone();
                async move { Ok::<_, Infallible>(handle_live_request(request, pool, stats, connect_timeout).await) }
            }),
        )
        .with_upgrades()
        .await
        .map_err(|err| err.to_string())
}

#[cfg(any(test, debug_assertions))]
async fn handle_test_request<F, Fut>(
    request: Request<Incoming>,
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connect_timeout: Duration,
    connector: F,
) -> Response<ProxyBody>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let account_name = match pool.next_account_name().await {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                "no ready session"
            );
            return empty_response(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    if request.method() == Method::CONNECT {
        let (host, port, target) = match resolve_connect_target(&request) {
            Ok(target) => target,
            Err(_) => return empty_response(StatusCode::BAD_REQUEST),
        };
        tracing::info!(
            protocol = tracing::field::display("connect"),
            target = %target,
            account = %account_name,
            "request accepted"
        );
        let on_upgrade = upgrade::on(request);
        let upstream = connector(account_name, host, port);
        let upstream = match connect_with_timeout(connect_timeout, upstream).await {
            Ok(upstream) => upstream,
            Err(err) => return gateway_error_response(&err),
        };
        let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Http));
        tokio::spawn(async move {
            let Ok(upgraded) = on_upgrade.await else {
                return;
            };
            let mut client = TokioIo::new(upgraded);
            let mut upstream = upstream;
            let _ = relay_upgraded_tunnel(&mut client, &mut upstream, connection.as_ref()).await;
        });
        return connect_established_response();
    }

    let (host, port, target, uri) = match resolve_forward_target(&request) {
        Ok(target) => target,
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
    };
    tracing::info!(
        protocol = tracing::field::display("http"),
        target = %target,
        account = %account_name,
        "request accepted"
    );

    let upstream = connector(account_name, host, port);
    let upstream = match connect_with_timeout(connect_timeout, upstream).await {
        Ok(upstream) => upstream,
        Err(err) => return gateway_error_response(&err),
    };
    let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Http));
    forward_request(request, uri, upstream, connection).await
}

async fn handle_live_request(
    request: Request<Incoming>,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
)-> Response<ProxyBody> {
    let (account_name, session) = match pool.next_live_session().await {
        Ok(ready) => ready,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                "no ready session"
            );
            return empty_response(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    if request.method() == Method::CONNECT {
        let (host, port, target) = match resolve_connect_target(&request) {
            Ok(target) => target,
            Err(_) => return empty_response(StatusCode::BAD_REQUEST),
        };
        tracing::info!(
            protocol = tracing::field::display("connect"),
            target = %target,
            account = %account_name,
            "request accepted"
        );
        let on_upgrade = upgrade::on(request);
        let upstream = session.connect_tcp((host.as_str(), port));
        let upstream = match connect_session_with_timeout(connect_timeout, upstream).await {
            Ok(upstream) => upstream,
            Err(err) => {
                if !matches!(
                    err,
                    UpstreamConnectError::RouteRejected | UpstreamConnectError::TimedOut
                ) {
                    stats.record_connect_failure();
                    pool.report_live_session_failure(&account_name, err.label())
                        .await;
                }
                return gateway_error_response(&err);
            }
        };
        stats.record_connect_success();
        let connection = stats.open_connection(ProxyProtocol::Http);
        tokio::spawn(async move {
            let Ok(upgraded) = on_upgrade.await else {
                return;
            };
            let mut client = TokioIo::new(upgraded);
            let mut upstream = upstream;
            let _ = relay_upgraded_tunnel(&mut client, &mut upstream, Some(&connection)).await;
        });
        return connect_established_response();
    }

    let (host, port, target, uri) = match resolve_forward_target(&request) {
        Ok(target) => target,
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
    };
    tracing::info!(
        protocol = tracing::field::display("http"),
        target = %target,
        account = %account_name,
        "request accepted"
    );

    let upstream = session.connect_tcp((host.as_str(), port));
    let upstream = match connect_session_with_timeout(connect_timeout, upstream).await {
        Ok(upstream) => upstream,
        Err(err) => {
            if !matches!(
                err,
                UpstreamConnectError::RouteRejected | UpstreamConnectError::TimedOut
            ) {
                stats.record_connect_failure();
                pool.report_live_session_failure(&account_name, err.label())
                    .await;
            }
            return gateway_error_response(&err);
        }
    };
    stats.record_connect_success();
    let connection = stats.open_connection(ProxyProtocol::Http);
    forward_request(request, uri, upstream, Some(connection)).await
}

fn empty_response(status: StatusCode) -> Response<ProxyBody> {
    let mut response = Response::new(empty_body());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response
}

fn connect_established_response() -> Response<ProxyBody> {
    let mut response = Response::new(empty_body());
    *response.status_mut() = StatusCode::OK;
    response
}

fn gateway_error_response(err: &UpstreamConnectError) -> Response<ProxyBody> {
    let status = match err {
        UpstreamConnectError::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        UpstreamConnectError::Failed(_) | UpstreamConnectError::RouteRejected => {
            StatusCode::BAD_GATEWAY
        }
    };
    empty_response(status)
}

fn resolve_forward_target(
    request: &Request<Incoming>,
) -> Result<(String, u16, String, Uri), String> {
    let authority = request
        .uri()
        .authority()
        .map(|authority| authority.as_str().to_string())
        .or_else(|| {
            request
                .headers()
                .get(HOST)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.to_string())
        })
        .ok_or_else(|| "missing host".to_string())?;
    let (host, port) = split_host_port(&authority, 80)?;
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let uri = path_and_query.parse::<Uri>().map_err(|err| err.to_string())?;
    let target = format!("{host}:{port}{path_and_query}");
    Ok((host.to_string(), port, target, uri))
}

fn resolve_connect_target(request: &Request<Incoming>) -> Result<(String, u16, String), String> {
    let target = request
        .uri()
        .authority()
        .map(|authority| authority.as_str().to_string())
        .or_else(|| {
            let path = request.uri().path();
            (!path.is_empty() && path != "/").then(|| path.to_string())
        })
        .ok_or_else(|| "missing connect authority".to_string())?;
    let (host, port) = split_host_port(&target, 443)?;
    Ok((host.to_string(), port, target))
}

async fn forward_request(
    request: Request<Incoming>,
    uri: Uri,
    mut upstream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    connection: Option<ConnectionGuard>,
) -> Response<ProxyBody> {
    let (parts, mut body) = request.into_parts();
    let mut upstream_request = format!(
        "{} {} {}\r\n",
        parts.method,
        uri,
        http_version_text(parts.version)
    );
    let mut forwarded_headers = http::HeaderMap::new();
    for (name, value) in &parts.headers {
        if should_strip_request_header(name) {
            continue;
        }
        forwarded_headers.insert(name.clone(), value.clone());
        upstream_request.push_str(name.as_str());
        upstream_request.push_str(": ");
        upstream_request.push_str(&String::from_utf8_lossy(value.as_bytes()));
        upstream_request.push_str("\r\n");
    }
    forwarded_headers.insert(CONNECTION, HeaderValue::from_static("close"));
    upstream_request.push_str("Connection: close\r\n\r\n");

    record_client_to_upstream(
        connection.as_ref(),
        estimate_request_size(
            &parts.method,
            &uri,
            parts.version,
            &forwarded_headers,
            0,
        ),
    );

    if upstream
        .write_all(upstream_request.as_bytes())
        .await
        .is_err()
    {
        return empty_response(StatusCode::BAD_GATEWAY);
    }

    let chunked_request = forwarded_headers
        .get(http::header::TRANSFER_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        });

    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return empty_response(StatusCode::BAD_GATEWAY);
        };
        if let Some(data) = frame.data_ref() {
            if chunked_request {
                let prefix = format!("{:X}\r\n", data.len());
                if upstream.write_all(prefix.as_bytes()).await.is_err()
                    || upstream.write_all(data).await.is_err()
                    || upstream.write_all(b"\r\n").await.is_err()
                {
                    return empty_response(StatusCode::BAD_GATEWAY);
                }
                record_client_to_upstream(connection.as_ref(), prefix.len() + data.len() + 2);
            } else {
                if upstream.write_all(data).await.is_err() {
                    return empty_response(StatusCode::BAD_GATEWAY);
                }
                record_client_to_upstream(connection.as_ref(), data.len());
            }
        }
    }
    if chunked_request {
        if upstream.write_all(b"0\r\n\r\n").await.is_err() {
            return empty_response(StatusCode::BAD_GATEWAY);
        }
        record_client_to_upstream(connection.as_ref(), 5);
    }

    match read_upstream_response(upstream, connection).await {
        Ok(response) => response,
        Err(_) => empty_response(StatusCode::BAD_GATEWAY),
    }
}

async fn relay_upgraded_tunnel(
    client: &mut (impl AsyncRead + AsyncWrite + Unpin),
    upstream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    connection: Option<&ConnectionGuard>,
) -> Result<(), String> {
    let (client_to_upstream, upstream_to_client) = copy_bidirectional(client, upstream)
        .await
        .map_err(|err| err.to_string())?;
    record_tunnel_transfer(connection, client_to_upstream, upstream_to_client);
    Ok(())
}

fn should_strip_request_header(name: &http::header::HeaderName) -> bool {
    let lower = name.as_str();
    lower.eq_ignore_ascii_case("proxy-connection")
        || name == PROXY_AUTHORIZATION
        || name == CONNECTION
        || lower.eq_ignore_ascii_case("keep-alive")
        || name == EXPECT
}

fn estimate_request_size(
    method: &Method,
    uri: &Uri,
    version: http::Version,
    headers: &http::HeaderMap<HeaderValue>,
    body_len: usize,
) -> usize {
    let request_line = format!(
        "{} {} {}\r\n",
        method.as_str(),
        uri,
        http_version_text(version)
    );
    request_line.len()
        + headers
            .iter()
            .map(|(name, value)| name.as_str().len() + 2 + value.as_bytes().len() + 2)
            .sum::<usize>()
        + 2
        + body_len
}

async fn read_upstream_response(
    mut upstream: impl AsyncRead + Unpin + Send + 'static,
    connection: Option<ConnectionGuard>,
) -> Result<Response<ProxyBody>, String> {
    let mut buffer = Vec::with_capacity(1024);
    let header_end = read_headers(&mut upstream, &mut buffer)
        .await
        .map_err(|err| err.to_string())?;
    let header_bytes = &buffer[..header_end];
    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
    let status_line = lines.next().ok_or_else(|| "missing status line".to_string())?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("invalid status line: {status_line}"))?;
    let header_lines: Vec<&str> = lines.collect();
    let body_kind = response_body_kind(status_code, &header_lines);
    let initial_body = buffer[header_end..].to_vec();
    let mut builder = Response::builder().status(status_code);
    for header in header_lines {
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("connection")
                || name.trim().eq_ignore_ascii_case("keep-alive")
                || (matches!(body_kind, ResponseBodyKind::Chunked)
                    && name.trim().eq_ignore_ascii_case("transfer-encoding"))
            {
                continue;
            }
            builder = builder.header(name.trim(), value.trim());
        }
    }
    builder = builder.header(CONNECTION, "close");
    let body = build_response_body(upstream, body_kind, initial_body, connection)?;
    builder.body(body).map_err(|err| err.to_string())
}

fn http_version_text(version: http::Version) -> &'static str {
    match version {
        http::Version::HTTP_09 => "HTTP/0.9",
        http::Version::HTTP_10 => "HTTP/1.0",
        http::Version::HTTP_11 => "HTTP/1.1",
        http::Version::HTTP_2 => "HTTP/2.0",
        http::Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/1.1",
    }
}

fn record_client_to_upstream(connection: Option<&ConnectionGuard>, bytes: usize) {
    if let Some(connection) = connection {
        connection.add_client_to_upstream_bytes(bytes as u64);
    }
}

fn record_tunnel_transfer(
    connection: Option<&ConnectionGuard>,
    client_to_upstream: u64,
    upstream_to_client: u64,
) {
    if let Some(connection) = connection {
        connection.add_client_to_upstream_bytes(client_to_upstream);
        connection.add_upstream_to_client_bytes(upstream_to_client);
    }
}

#[cfg(any(test, debug_assertions))]
async fn connect_with_timeout<T, E, Fut>(
    timeout: Duration,
    fut: Fut,
) -> Result<T, UpstreamConnectError>
where
    E: std::fmt::Debug,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(UpstreamConnectError::Failed(format!("{err:?}"))),
        Err(_) => Err(UpstreamConnectError::TimedOut),
    }
}

async fn connect_session_with_timeout<Fut>(
    timeout: Duration,
    fut: Fut,
) -> Result<smelly_connect::transport::VpnStream, UpstreamConnectError>
where
    Fut: std::future::Future<Output = Result<smelly_connect::transport::VpnStream, smelly_connect::Error>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed,
        ))) => Err(UpstreamConnectError::RouteRejected),
        Ok(Err(smelly_connect::Error::Transport(
            smelly_connect::error::TransportError::ConnectFailed(message),
        ))) if message.to_ascii_lowercase().contains("timed out") => {
            Err(UpstreamConnectError::TimedOut)
        }
        Ok(Err(err)) => Err(UpstreamConnectError::Failed(format!("{err:?}"))),
        Err(_) => Err(UpstreamConnectError::TimedOut),
    }
}

fn build_response_body(
    upstream: impl AsyncRead + Unpin + Send + 'static,
    body_kind: ResponseBodyKind,
    initial_body: Vec<u8>,
    connection: Option<ConnectionGuard>,
) -> Result<ProxyBody, String> {
    match body_kind {
        ResponseBodyKind::None => Ok(empty_body()),
        ResponseBodyKind::ContentLength(length) => {
            if initial_body.len() >= length {
                let body = initial_body[..length].to_vec();
                Ok(full_body(body, connection))
            } else {
                let (tx, rx) = mpsc::channel(1);
                stream_content_length_body(upstream, initial_body, length, tx);
                Ok(CountedBody::new(ChannelBody::new(rx), connection).boxed())
            }
        }
        ResponseBodyKind::Chunked => {
            let (tx, rx) = mpsc::channel(1);
            stream_chunked_body(upstream, initial_body, tx);
            Ok(CountedBody::new(ChannelBody::new(rx), connection).boxed())
        }
        ResponseBodyKind::ReadToEnd => {
            let (tx, rx) = mpsc::channel(1);
            stream_read_to_end_body(upstream, initial_body, tx);
            Ok(CountedBody::new(ChannelBody::new(rx), connection).boxed())
        }
    }
}

fn stream_content_length_body(
    upstream: impl AsyncRead + Unpin + Send + 'static,
    initial_body: Vec<u8>,
    length: usize,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    tokio::spawn(async move {
        let mut upstream = upstream;
        let mut remaining = length;
        if !initial_body.is_empty() {
            let initial_len = remaining.min(initial_body.len());
            let initial = initial_body[..initial_len].to_vec();
            if tx.send(Ok(Bytes::from(initial))).await.is_err() {
                return;
            }
            remaining -= initial_len;
        }
        if remaining == 0 {
            return;
        }
        let mut chunk = [0_u8; 8192];
        while remaining > 0 {
            let limit = remaining.min(chunk.len());
            let n = match upstream.read(&mut chunk[..limit]).await {
                Ok(n) => n,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };
            if n == 0 {
                let _ = tx
                    .send(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed before response body completed",
                    )))
                    .await;
                return;
            }
            remaining -= n;
            if tx
                .send(Ok(Bytes::copy_from_slice(&chunk[..n])))
                .await
                .is_err()
            {
                return;
            }
        }
    });
}

fn stream_read_to_end_body(
    upstream: impl AsyncRead + Unpin + Send + 'static,
    initial_body: Vec<u8>,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    tokio::spawn(async move {
        if !initial_body.is_empty() && tx.send(Ok(Bytes::from(initial_body))).await.is_err() {
            return;
        }
        let mut chunk = [0_u8; 8192];
        let mut upstream = upstream;
        loop {
            let n = match upstream.read(&mut chunk).await {
                Ok(n) => n,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };
            if n == 0 {
                return;
            }
            if tx
                .send(Ok(Bytes::copy_from_slice(&chunk[..n])))
                .await
                .is_err()
            {
                return;
            }
        }
    });
}

fn stream_chunked_body(
    upstream: impl AsyncRead + Unpin + Send + 'static,
    initial_body: Vec<u8>,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    tokio::spawn(async move {
        let mut decoder = ChunkedResponseDecoder::new();
        match decoder.feed(&initial_body) {
            Ok(decoded) => {
                for chunk in decoded {
                    if tx.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
            }
            Err(err) => {
                let _ = tx.send(Err(err)).await;
                return;
            }
        }
        if decoder.is_done() {
            return;
        }

        let mut chunk = [0_u8; 8192];
        let mut upstream = upstream;
        loop {
            let n = match upstream.read(&mut chunk).await {
                Ok(n) => n,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };
            if n == 0 {
                let _ = tx
                    .send(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed before chunked response completed",
                    )))
                    .await;
                return;
            }
            match decoder.feed(&chunk[..n]) {
                Ok(decoded) => {
                    for chunk in decoded {
                        if tx.send(Ok(chunk)).await.is_err() {
                            return;
                        }
                    }
                    if decoder.is_done() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            }
        }
    });
}

fn empty_body() -> ProxyBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn full_body(body: Vec<u8>, connection: Option<ConnectionGuard>) -> ProxyBody {
    CountedBody::new(
        Full::new(Bytes::from(body))
            .map_err(|never| match never {}),
        connection,
    )
    .boxed()
}

fn response_body_kind(status_code: u16, header_lines: &[&str]) -> ResponseBodyKind {
    if (100..200).contains(&status_code) || status_code == 204 || status_code == 304 {
        ResponseBodyKind::None
    } else if has_chunked_transfer_encoding(header_lines) {
        ResponseBodyKind::Chunked
    } else if let Some(content_length) = parse_content_length(header_lines) {
        ResponseBodyKind::ContentLength(content_length)
    } else {
        ResponseBodyKind::ReadToEnd
    }
}

async fn read_headers(
    stream: &mut (impl AsyncRead + Unpin),
    buffer: &mut Vec<u8>,
) -> io::Result<usize> {
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

enum ChunkedState {
    SizeLine(Vec<u8>),
    Data(usize),
    DataCrLf(usize),
    Trailers(Vec<u8>),
    Done,
}

fn split_host_port(target: &str, default_port: u16) -> Result<(&str, u16), String> {
    if let Some((host, port)) = target.rsplit_once(':') {
        let port = port.parse().map_err(|_| "invalid port".to_string())?;
        Ok((host, port))
    } else {
        Ok((target, default_port))
    }
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
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
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    });
    addr
}

#[cfg(any(test, debug_assertions))]
async fn spawn_request_body_echo_upstream() -> SocketAddr {
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
                header_end = find_header_end(&request);
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
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
        let status = if body_complete { "200 OK" } else { "400 Bad Request" };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    addr
}

#[cfg(any(test, debug_assertions))]
async fn spawn_chunked_request_body_echo_upstream() -> SocketAddr {
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

#[cfg(any(test, debug_assertions))]
async fn spawn_proxy_auth_capture_upstream() -> SocketAddr {
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
            if find_header_end(&request).is_some() {
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

#[cfg(any(test, debug_assertions))]
async fn spawn_slow_streaming_response_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            if n == 0 {
                return;
            }
            request.extend_from_slice(&chunk[..n]);
            if find_header_end(&request).is_some() {
                break;
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        socket.write_all(b" world").await.unwrap();
    });
    addr
}

#[cfg(any(test, debug_assertions))]
fn chunked_request_complete(request: &[u8]) -> bool {
    let Some(header_end) = find_header_end(request) else {
        return false;
    };
    let body = &request[header_end..];
    chunked_wire_complete(body)
}

#[cfg(any(test, debug_assertions))]
fn extract_chunked_request_body(request: &[u8]) -> Option<String> {
    let header_end = find_header_end(request)?;
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

#[cfg(any(test, debug_assertions))]
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
        let Ok(size) = usize::from_str_radix(
            size_line.split(';').next().unwrap_or_default().trim(),
            16,
        ) else {
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

#[cfg(any(test, debug_assertions))]
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
