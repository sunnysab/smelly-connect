use std::convert::Infallible;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{CONNECTION, EXPECT, HOST, PROXY_AUTHORIZATION};
use http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::{Body as HyperBody, Frame, Incoming};
use hyper::server::conn::http1 as hyper_server_http1;
use hyper::service::service_fn;
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;

use crate::pool::SessionPool;
use crate::runtime::{ConnectionGuard, ProxyProtocol, RuntimeStats};
use smelly_connect::proxy::http::{
    find_header_end, has_chunked_transfer_encoding, parse_content_length,
};

#[cfg(feature = "test-utils")]
use super::common::connect_with_timeout;
use super::common::{
    LISTENER_ACCEPT_RETRY_BACKOFF, ListenerAcceptRetryLogState, LiveRouteBackend,
    UpstreamConnectError, connect_planned_live_upstream_with_timeout, log_request_accepted,
    plan_live_upstream_connect, should_retry_listener_accept,
};

type ProxyBody = BoxBody<Bytes, io::Error>;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_IN_FLIGHT_CONNECTIONS: usize = 1024;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(feature = "test-utils")]
pub use tests::HttpProxyTestResult;

#[cfg(feature = "test-utils")]
pub use tests::HttpBodyTestResult;

#[cfg(feature = "test-utils")]
pub use tests::ReusedUpstreamTestResult;

#[cfg(feature = "test-utils")]
pub use tests::StreamingResponseTestResult;

#[cfg(feature = "test-utils")]
pub use tests::ConnectProxyTestResult;

#[cfg(feature = "test-utils")]
pub use tests::NoReadySessionResult;

#[cfg(feature = "test-utils")]
pub use tests::HttpStatusBodyTestResult;

#[cfg(feature = "test-utils")]
pub use tests::TimeoutTestResult;

#[cfg(feature = "test-utils")]
pub use tests::LiveFailureRecoveryTestResult;

#[cfg(feature = "test-utils")]
pub use tests::LiveFailureLatencyTestResult;

enum ResponseBodyKind {
    None,
    ContentLength(usize),
    Chunked,
    ReadToEnd,
}

struct ParsedResponseHead {
    status_code: u16,
    body_kind: ResponseBodyKind,
    can_reuse: bool,
    forwarded_headers: Vec<(HeaderName, HeaderValue)>,
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

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_origin_form_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_origin_form_ipv6_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_body_completes_for_keep_alive_upstream_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_reuses_upstream_connection_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_cached_vpn_reuse_failure_recovers_live_session_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_cached_vpn_reuse_failure_recovers_live_session_with_keep_alive_request_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_streams_request_body_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_streams_chunked_request_body_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_expect_continue_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_strips_proxy_authorization_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_preserves_non_utf8_request_header_bytes_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_preserves_non_utf8_response_header_bytes_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_streams_response_body_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_head_response_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_reuses_upstream_connection_after_head_response_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_rejects_oversized_response_headers_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_connect_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_direct_forward_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_connect_direct_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_live_vpn_connect_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_live_vpn_forward_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_direct_failure_does_not_open_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_no_ready_session_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_no_ready_session_sequence_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_runtime_stats_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_connect_failure_runtime_status_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_cached_reuse_success_preserves_runtime_status_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_connect_timeout_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_connect_failure_status_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_connect_timeout_status_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_live_connect_failure_recovery_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_live_connect_failure_does_not_wait_for_probe_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_route_rejection_does_not_open_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_timeout_does_not_open_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_immediate_timeout_status_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_allow_all_failure_does_not_open_for_test;

pub async fn serve_http(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
) -> Result<(), String> {
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    serve_http_with_shutdown(listen, pool, stats, connect_timeout, shutdown_rx).await
}

pub async fn serve_http_with_shutdown(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    serve_http_with_limit(
        listen,
        pool,
        stats,
        connect_timeout,
        DEFAULT_MAX_IN_FLIGHT_CONNECTIONS,
        shutdown,
    )
    .await
}

async fn serve_http_with_limit(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    max_in_flight_connections: usize,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|err| err.to_string())?;
    let local_addr = listener.local_addr().map_err(|err| err.to_string())?;
    let limiter = Arc::new(Semaphore::new(max_in_flight_connections));
    let mut clients = JoinSet::new();
    let mut accept_retry_log_state = ListenerAcceptRetryLogState::default();
    let mut shutting_down = *shutdown.borrow();
    tracing::info!(
        protocol = tracing::field::display("http"),
        listen = %local_addr,
        "http proxy listening"
    );
    loop {
        if shutting_down && clients.is_empty() {
            break;
        }

        tokio::select! {
            changed = shutdown.changed(), if !shutting_down => {
                match changed {
                    Ok(()) | Err(_) => shutting_down = true,
                }
            }
            result = clients.join_next(), if !clients.is_empty() => {
                if let Some(Err(err)) = result {
                    tracing::warn!(
                        protocol = tracing::field::display("http"),
                        error = %err,
                        "http connection task failed"
                    );
                }
            }
            accepted = listener.accept(), if !shutting_down => {
                let (stream, _) = match accepted {
                    Ok(accepted) => {
                        accept_retry_log_state.reset();
                        accepted
                    }
                    Err(err) if should_retry_listener_accept(&err) => {
                        if accept_retry_log_state.should_warn() {
                            tracing::warn!(
                                protocol = tracing::field::display("http"),
                                error = %err,
                                backoff_ms = LISTENER_ACCEPT_RETRY_BACKOFF.as_millis() as u64,
                                "http listener accept hit transient fd exhaustion; retrying"
                            );
                        }
                        tokio::time::sleep(LISTENER_ACCEPT_RETRY_BACKOFF).await;
                        continue;
                    }
                    Err(err) => return Err(err.to_string()),
                };
                let permit = limiter.clone().try_acquire_owned();
                let pool = pool.clone();
                let stats = stats.clone();
                match permit {
                    Ok(permit) => {
                        clients.spawn(async move {
                            let _permit = permit;
                            if let Err(err) = handle_live_client(stream, pool, stats, connect_timeout).await
                            {
                                tracing::warn!(
                                    protocol = tracing::field::display("http"),
                                    error = %err,
                                    "live proxy request failed"
                                );
                            }
                        });
                    }
                    Err(_) => {
                        stats.record_service_unavailable_over_capacity(ProxyProtocol::Http);
                        clients.spawn(async move {
                            let _ = reject_over_capacity_http(stream).await;
                        });
                    }
                }
            }
        }
    }
    while let Some(result) = clients.join_next().await {
        if let Err(err) = result {
            tracing::warn!(
                protocol = tracing::field::display("http"),
                error = %err,
                "http connection task failed during shutdown"
            );
        }
    }
    Ok(())
}

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_live_failure_for_test;

#[cfg(feature = "test-utils")]
pub use tests::proxy_http_over_capacity_for_test;

async fn reject_over_capacity_http(mut stream: TcpStream) -> io::Result<()> {
    stream
        .write_all(
            b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        )
        .await?;
    stream.shutdown().await
}

async fn handle_live_client(
    client: TcpStream,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
) -> Result<(), String> {
    let upstream_cache = Arc::new(tokio::sync::Mutex::new(
        None::<CachedUpstream<smelly_connect::transport::VpnStream, CachedLiveUpstreamMetadata>>,
    ));
    let io = TokioIo::new(client);
    hyper_server_http1::Builder::new()
        .half_close(true)
        .serve_connection(
            io,
            service_fn(move |request| {
                let pool = pool.clone();
                let stats = stats.clone();
                let upstream_cache = Arc::clone(&upstream_cache);
                async move {
                    Ok::<_, Infallible>(
                        handle_live_request(request, pool, stats, connect_timeout, upstream_cache)
                            .await,
                    )
                }
            }),
        )
        .with_upgrades()
        .await
        .map_err(|err| err.to_string())
}

async fn handle_live_request(
    request: Request<Incoming>,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    upstream_cache: Arc<
        tokio::sync::Mutex<
            Option<
                CachedUpstream<smelly_connect::transport::VpnStream, CachedLiveUpstreamMetadata>,
            >,
        >,
    >,
) -> Response<ProxyBody> {
    let request_id = next_request_id();
    let (account_name, session) = match pool.next_live_session().await {
        Ok(ready) => ready,
        Err(_) => {
            log_no_ready_session(request_id, "http");
            stats.record_service_unavailable_no_ready_session(ProxyProtocol::Http);
            return empty_response(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    if request.method() == Method::CONNECT {
        let (host, port, target) = match resolve_connect_target(&request) {
            Ok(target) => target,
            Err(_) => return empty_response(StatusCode::BAD_REQUEST),
        };
        let route_plan = match plan_live_upstream_connect(&session, &host, port).await {
            Ok(route_plan) => route_plan,
            Err((err, route_backend)) => {
                if !matches!(err, UpstreamConnectError::RouteRejected) {
                    stats.record_connect_failure();
                }
                if matches!(route_backend, LiveRouteBackend::Vpn) {
                    handle_live_session_failure(&pool, &account_name, &session, &err).await;
                }
                return gateway_error_response(&err);
            }
        };
        let route_backend = route_plan.backend();
        log_request_accepted(
            Some(request_id),
            "connect",
            &target,
            route_backend,
            &account_name,
        );
        let on_upgrade = upgrade::on(request);
        let connect_started = Instant::now();
        log_upstream_connect_start(
            request_id,
            "connect",
            route_backend,
            route_account(route_backend, &account_name),
            &target,
            connect_timeout,
        );
        let (upstream, _route_backend) = match connect_planned_live_upstream_with_timeout(
            connect_timeout,
            &session,
            &host,
            port,
            route_plan,
        )
        .await
        {
            Ok((upstream, route_backend)) => {
                log_upstream_connect_success(request_id, "connect", &target, connect_started);
                (upstream, route_backend)
            }
            Err((err, route_backend)) => {
                log_upstream_connect_failure(request_id, "connect", &target, connect_started, &err);
                if !matches!(err, UpstreamConnectError::RouteRejected) {
                    stats.record_connect_failure();
                }
                if matches!(route_backend, LiveRouteBackend::Vpn) {
                    handle_live_session_failure(&pool, &account_name, &session, &err).await;
                }
                return gateway_error_response(&err);
            }
        };
        stats.record_connect_success();
        let connection = stats.open_connection(ProxyProtocol::Http);
        tokio::spawn(async move {
            let Ok(upgraded) = on_upgrade.await else {
                tracing::warn!(request_id, target = %target, "http connect upgrade failed");
                return;
            };
            tracing::info!(request_id, target = %target, "http connect tunnel established");
            let mut client = TokioIo::new(upgraded);
            let mut upstream = upstream;
            let relay_started = Instant::now();
            match relay_upgraded_tunnel(&mut client, &mut upstream, Some(&connection)).await {
                Ok((client_to_upstream_bytes, upstream_to_client_bytes)) => {
                    tracing::info!(
                        request_id,
                        target = %target,
                        elapsed_ms = elapsed_ms(relay_started),
                        client_to_upstream_bytes,
                        upstream_to_client_bytes,
                        "http connect tunnel relay finished"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        request_id,
                        target = %target,
                        elapsed_ms = elapsed_ms(relay_started),
                        error = %err,
                        "http connect tunnel relay failed"
                    );
                }
            }
        });
        return connect_established_response();
    }

    let (host, port, target, uri) = match resolve_forward_target(&request) {
        Ok(target) => target,
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
    };

    let wants_keep_alive = client_requests_keep_alive(&request);
    let upstream = take_cached_upstream(&upstream_cache, &host, port).await;
    let upstream = match upstream {
        Some(upstream) => {
            let CachedUpstream {
                stream,
                metadata,
                host: _,
                port: _,
            } = upstream;
            log_request_accepted(
                Some(request_id),
                "http",
                &target,
                metadata.route_backend,
                &metadata.account_name,
            );
            Ok((stream, metadata, true))
        }
        None => {
            let route_plan = match plan_live_upstream_connect(&session, &host, port).await {
                Ok(route_plan) => route_plan,
                Err((err, route_backend)) => {
                    if !matches!(err, UpstreamConnectError::RouteRejected) {
                        stats.record_connect_failure();
                    }
                    if matches!(route_backend, LiveRouteBackend::Vpn) {
                        handle_live_session_failure(&pool, &account_name, &session, &err).await;
                    }
                    return gateway_error_response(&err);
                }
            };
            let route_backend = route_plan.backend();
            log_request_accepted(
                Some(request_id),
                "http",
                &target,
                route_backend,
                &account_name,
            );
            let connect_started = Instant::now();
            log_upstream_connect_start(
                request_id,
                "http",
                route_backend,
                route_account(route_backend, &account_name),
                &target,
                connect_timeout,
            );
            match connect_planned_live_upstream_with_timeout(
                connect_timeout,
                &session,
                &host,
                port,
                route_plan,
            )
            .await
            {
                Ok((upstream, route_backend)) => {
                    log_upstream_connect_success(request_id, "http", &target, connect_started);
                    Ok((
                        upstream,
                        CachedLiveUpstreamMetadata {
                            route_backend,
                            account_name: account_name.clone(),
                            session: session.clone(),
                        },
                        false,
                    ))
                }
                Err((err, route_backend)) => {
                    log_upstream_connect_failure(
                        request_id,
                        "http",
                        &target,
                        connect_started,
                        &err,
                    );
                    Err((err, route_backend))
                }
            }
        }
    };
    let (upstream, cache_metadata, reused_cached_upstream) = match upstream {
        Ok(upstream) => upstream,
        Err((err, route_backend)) => {
            if !matches!(err, UpstreamConnectError::RouteRejected) {
                stats.record_connect_failure();
            }
            if matches!(route_backend, LiveRouteBackend::Vpn) {
                handle_live_session_failure(&pool, &account_name, &session, &err).await;
            }
            return gateway_error_response(&err);
        }
    };
    if !reused_cached_upstream {
        stats.record_connect_success();
    }
    let connection = stats.open_connection(ProxyProtocol::Http);
    if wants_keep_alive {
        let (response, reusable, reuse_error) =
            forward_request_with_reuse(request, uri, upstream, Some(connection)).await;
        if let Some(err) = reuse_error {
            handle_cached_live_upstream_failure(&pool, &stats, &cache_metadata, &err).await;
        }
        if let Some(reusable) = reusable {
            store_cached_upstream(
                &upstream_cache,
                CachedUpstream {
                    host,
                    port,
                    stream: reusable,
                    metadata: cache_metadata,
                },
            )
            .await;
        }
        response
    } else {
        let (response, forward_error) =
            forward_request(request, uri, upstream, Some(connection)).await;
        if reused_cached_upstream && let Some(err) = forward_error {
            handle_cached_live_upstream_failure(&pool, &stats, &cache_metadata, &err).await;
        }
        response
    }
}

struct CachedUpstream<S, M = ()> {
    host: String,
    port: u16,
    stream: S,
    metadata: M,
}

struct CachedLiveUpstreamMetadata {
    route_backend: LiveRouteBackend,
    account_name: String,
    session: smelly_connect::Session,
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
    empty_response(gateway_error_status(err))
}

async fn handle_live_session_failure(
    pool: &SessionPool,
    account_name: &str,
    session: &smelly_connect::Session,
    err: &UpstreamConnectError,
) {
    if matches!(err, UpstreamConnectError::Failed) {
        pool.report_live_session_unhealthy_if_probe_fails(
            account_name,
            session,
            format!("{err:?}"),
        )
        .await;
    }
}

async fn handle_cached_live_upstream_failure(
    pool: &SessionPool,
    stats: &RuntimeStats,
    cache_metadata: &CachedLiveUpstreamMetadata,
    err: &UpstreamConnectError,
) {
    stats.record_connect_failure();
    if matches!(cache_metadata.route_backend, LiveRouteBackend::Vpn) {
        handle_live_session_failure(
            pool,
            &cache_metadata.account_name,
            &cache_metadata.session,
            err,
        )
        .await;
    }
}

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn gateway_error_status(err: &UpstreamConnectError) -> StatusCode {
    match err {
        UpstreamConnectError::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        UpstreamConnectError::RouteRejected => StatusCode::FORBIDDEN,
        UpstreamConnectError::Failed => StatusCode::BAD_GATEWAY,
    }
}

fn upstream_error_label(err: &UpstreamConnectError) -> &'static str {
    match err {
        UpstreamConnectError::TimedOut => "timed_out",
        UpstreamConnectError::RouteRejected => "route_rejected",
        UpstreamConnectError::Failed => "failed",
    }
}

fn log_no_ready_session(request_id: u64, protocol: &'static str) {
    tracing::warn!(
        request_id,
        protocol = tracing::field::display(protocol),
        "no ready session"
    );
}

fn route_account(route_backend: LiveRouteBackend, account: &str) -> Option<&str> {
    matches!(route_backend, LiveRouteBackend::Vpn).then_some(account)
}

fn log_upstream_connect_start(
    request_id: u64,
    protocol: &'static str,
    route_backend: LiveRouteBackend,
    account: Option<&str>,
    target: &str,
    timeout: Duration,
) {
    match account {
        Some(account) => tracing::info!(
            request_id,
            protocol = tracing::field::display(protocol),
            route = %super::common::live_route_label(route_backend),
            account = %account,
            target = %target,
            timeout_ms = timeout.as_millis().try_into().unwrap_or(u64::MAX),
            "http upstream connect start"
        ),
        None => tracing::info!(
            request_id,
            protocol = tracing::field::display(protocol),
            route = %super::common::live_route_label(route_backend),
            target = %target,
            timeout_ms = timeout.as_millis().try_into().unwrap_or(u64::MAX),
            "http upstream connect start"
        ),
    }
}

fn log_upstream_connect_success(
    request_id: u64,
    protocol: &'static str,
    target: &str,
    started: Instant,
) {
    tracing::info!(
        request_id,
        protocol = tracing::field::display(protocol),
        target,
        elapsed_ms = elapsed_ms(started),
        result = "ok",
        "http upstream connect result"
    );
}

fn log_upstream_connect_failure(
    request_id: u64,
    protocol: &'static str,
    target: &str,
    started: Instant,
    err: &UpstreamConnectError,
) {
    tracing::warn!(
        request_id,
        protocol = tracing::field::display(protocol),
        target,
        elapsed_ms = elapsed_ms(started),
        result = upstream_error_label(err),
        http_status = gateway_error_status(err).as_u16(),
        "http upstream connect result"
    );
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
    let uri = path_and_query
        .parse::<Uri>()
        .map_err(|err| err.to_string())?;
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
) -> (Response<ProxyBody>, Option<UpstreamConnectError>) {
    let (parts, mut body) = request.into_parts();
    let request_allows_response_body = parts.method != Method::HEAD;
    let mut upstream_request = format!(
        "{} {} {}\r\n",
        parts.method,
        uri,
        http_version_text(parts.version)
    )
    .into_bytes();
    let mut forwarded_headers = http::HeaderMap::new();
    for (name, value) in &parts.headers {
        if should_strip_request_header(name) {
            continue;
        }
        forwarded_headers.insert(name.clone(), value.clone());
        push_header_line(&mut upstream_request, name, value);
    }
    forwarded_headers.insert(CONNECTION, HeaderValue::from_static("close"));
    upstream_request.extend_from_slice(b"Connection: close\r\n\r\n");

    record_client_to_upstream(
        connection.as_ref(),
        estimate_request_size(&parts.method, &uri, parts.version, &forwarded_headers, 0),
    );

    if upstream.write_all(&upstream_request).await.is_err() {
        return (
            empty_response(StatusCode::BAD_GATEWAY),
            Some(UpstreamConnectError::Failed),
        );
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
            return (empty_response(StatusCode::BAD_GATEWAY), None);
        };
        if let Some(data) = frame.data_ref() {
            if chunked_request {
                let prefix = format!("{:X}\r\n", data.len());
                if upstream.write_all(prefix.as_bytes()).await.is_err()
                    || upstream.write_all(data).await.is_err()
                    || upstream.write_all(b"\r\n").await.is_err()
                {
                    return (
                        empty_response(StatusCode::BAD_GATEWAY),
                        Some(UpstreamConnectError::Failed),
                    );
                }
                record_client_to_upstream(connection.as_ref(), prefix.len() + data.len() + 2);
            } else {
                if upstream.write_all(data).await.is_err() {
                    return (
                        empty_response(StatusCode::BAD_GATEWAY),
                        Some(UpstreamConnectError::Failed),
                    );
                }
                record_client_to_upstream(connection.as_ref(), data.len());
            }
        }
    }
    if chunked_request {
        if upstream.write_all(b"0\r\n\r\n").await.is_err() {
            return (
                empty_response(StatusCode::BAD_GATEWAY),
                Some(UpstreamConnectError::Failed),
            );
        }
        record_client_to_upstream(connection.as_ref(), 5);
    }

    match read_upstream_response(upstream, connection, request_allows_response_body).await {
        Ok(response) => (response, None),
        Err(_) => (
            empty_response(StatusCode::BAD_GATEWAY),
            Some(UpstreamConnectError::Failed),
        ),
    }
}

async fn forward_request_with_reuse<S>(
    request: Request<Incoming>,
    uri: Uri,
    mut upstream: S,
    connection: Option<ConnectionGuard>,
) -> (Response<ProxyBody>, Option<S>, Option<UpstreamConnectError>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (parts, mut body) = request.into_parts();
    let request_allows_response_body = parts.method != Method::HEAD;
    let mut upstream_request = format!(
        "{} {} {}\r\n",
        parts.method,
        uri,
        http_version_text(parts.version)
    )
    .into_bytes();
    let mut forwarded_headers = http::HeaderMap::new();
    for (name, value) in &parts.headers {
        if should_strip_request_header(name) {
            continue;
        }
        forwarded_headers.insert(name.clone(), value.clone());
        push_header_line(&mut upstream_request, name, value);
    }
    upstream_request.extend_from_slice(b"\r\n");

    record_client_to_upstream(
        connection.as_ref(),
        estimate_request_size(&parts.method, &uri, parts.version, &forwarded_headers, 0),
    );

    if upstream.write_all(&upstream_request).await.is_err() {
        return (
            empty_response(StatusCode::BAD_GATEWAY),
            None,
            Some(UpstreamConnectError::Failed),
        );
    }

    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return (empty_response(StatusCode::BAD_GATEWAY), None, None);
        };
        if let Some(data) = frame.data_ref() {
            if upstream.write_all(data).await.is_err() {
                return (
                    empty_response(StatusCode::BAD_GATEWAY),
                    None,
                    Some(UpstreamConnectError::Failed),
                );
            }
            record_client_to_upstream(connection.as_ref(), data.len());
        }
    }

    match read_reusable_upstream_response(upstream, connection, request_allows_response_body).await
    {
        Ok((response, reusable)) => (response, reusable, None),
        Err(_) => (
            empty_response(StatusCode::BAD_GATEWAY),
            None,
            Some(UpstreamConnectError::Failed),
        ),
    }
}

async fn relay_upgraded_tunnel(
    client: &mut (impl AsyncRead + AsyncWrite + Unpin),
    upstream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    connection: Option<&ConnectionGuard>,
) -> Result<(u64, u64), String> {
    let (client_to_upstream, upstream_to_client) = copy_bidirectional(client, upstream)
        .await
        .map_err(|err| err.to_string())?;
    record_tunnel_transfer(connection, client_to_upstream, upstream_to_client);
    Ok((client_to_upstream, upstream_to_client))
}

fn should_strip_request_header(name: &http::header::HeaderName) -> bool {
    let lower = name.as_str();
    lower.eq_ignore_ascii_case("proxy-connection")
        || name == PROXY_AUTHORIZATION
        || name == CONNECTION
        || lower.eq_ignore_ascii_case("keep-alive")
        || name == EXPECT
}

fn client_requests_keep_alive(request: &Request<Incoming>) -> bool {
    request
        .headers()
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("keep-alive"))
        })
}

async fn take_cached_upstream<S, M>(
    cache: &tokio::sync::Mutex<Option<CachedUpstream<S, M>>>,
    host: &str,
    port: u16,
) -> Option<CachedUpstream<S, M>> {
    let mut cache = cache.lock().await;
    let cached = cache.take()?;
    if cached.host == host && cached.port == port {
        Some(cached)
    } else {
        None
    }
}

async fn store_cached_upstream<S, M>(
    cache: &tokio::sync::Mutex<Option<CachedUpstream<S, M>>>,
    cached: CachedUpstream<S, M>,
) {
    let mut cache = cache.lock().await;
    *cache = Some(cached);
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
    request_allows_response_body: bool,
) -> Result<Response<ProxyBody>, String> {
    let mut buffer = Vec::with_capacity(1024);
    let header_end = read_headers(&mut upstream, &mut buffer)
        .await
        .map_err(|err| err.to_string())?;
    let header_bytes = &buffer[..header_end];
    let head = parse_upstream_response_head(header_bytes, request_allows_response_body)?;
    let initial_body = buffer[header_end..].to_vec();
    let mut builder = Response::builder().status(head.status_code);
    for (name, value) in head.forwarded_headers {
        builder = builder.header(name, value);
    }
    builder = builder.header(CONNECTION, "close");
    let body = build_response_body(upstream, head.body_kind, initial_body, connection)?;
    builder.body(body).map_err(|err| err.to_string())
}

async fn read_reusable_upstream_response<S>(
    mut upstream: S,
    connection: Option<ConnectionGuard>,
    request_allows_response_body: bool,
) -> Result<(Response<ProxyBody>, Option<S>), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut buffer = Vec::with_capacity(1024);
    let header_end = read_headers(&mut upstream, &mut buffer)
        .await
        .map_err(|err| err.to_string())?;
    let header_bytes = &buffer[..header_end];
    let head = parse_upstream_response_head(header_bytes, request_allows_response_body)?;
    let initial_body = buffer[header_end..].to_vec();

    if !head.can_reuse {
        let mut builder = Response::builder().status(head.status_code);
        for (name, value) in head.forwarded_headers {
            builder = builder.header(name, value);
        }
        builder = builder.header(CONNECTION, "close");
        let body = build_response_body(upstream, head.body_kind, initial_body, connection)?;
        let response = builder.body(body).map_err(|err| err.to_string())?;
        return Ok((response, None));
    }

    let body = match head.body_kind {
        ResponseBodyKind::None => Vec::new(),
        ResponseBodyKind::ContentLength(length) => {
            let mut body = initial_body;
            while body.len() < length {
                let mut chunk = [0_u8; 8192];
                let n = upstream
                    .read(&mut chunk)
                    .await
                    .map_err(|err| err.to_string())?;
                if n == 0 {
                    return Err(
                        "connection closed before reusable response body completed".to_string()
                    );
                }
                body.extend_from_slice(&chunk[..n]);
            }
            body.truncate(length);
            body
        }
        ResponseBodyKind::Chunked | ResponseBodyKind::ReadToEnd => unreachable!(),
    };

    if let Some(connection) = &connection {
        connection.add_upstream_to_client_bytes(body.len() as u64);
    }

    let mut builder = Response::builder().status(head.status_code);
    for (name, value) in head.forwarded_headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .body(full_body(body, connection))
        .map_err(|err| err.to_string())?;
    Ok((response, Some(upstream)))
}

fn response_allows_reuse(header_lines: &[&str]) -> bool {
    !header_lines.iter().any(|header| {
        header.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("connection")
                && value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("close"))
        })
    })
}

fn push_header_line(buffer: &mut Vec<u8>, name: &HeaderName, value: &HeaderValue) {
    buffer.extend_from_slice(name.as_str().as_bytes());
    buffer.extend_from_slice(b": ");
    buffer.extend_from_slice(value.as_bytes());
    buffer.extend_from_slice(b"\r\n");
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

fn parse_upstream_response_head(
    header_bytes: &[u8],
    request_allows_response_body: bool,
) -> Result<ParsedResponseHead, String> {
    let mut lines = header_bytes
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty());
    let status_line = lines
        .next()
        .ok_or_else(|| "missing status line".to_string())?;
    let status_line =
        std::str::from_utf8(status_line).map_err(|_| "invalid status line encoding".to_string())?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("invalid status line: {status_line}"))?;

    let mut forwarded_headers = Vec::new();
    let mut ascii_header_lines = Vec::new();
    for header in lines {
        let Some(separator) = header.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let name_bytes = trim_ascii_http_whitespace(&header[..separator]);
        let value_bytes = trim_ascii_http_whitespace(&header[separator + 1..]);
        let name = HeaderName::from_bytes(name_bytes).map_err(|err| err.to_string())?;
        let value = HeaderValue::from_bytes(value_bytes).map_err(|err| err.to_string())?;
        if let Ok(value_text) = value.to_str() {
            ascii_header_lines.push(format!("{}: {value_text}", name.as_str()));
        }
        forwarded_headers.push((name, value));
    }

    let header_lines: Vec<&str> = ascii_header_lines.iter().map(String::as_str).collect();
    let body_kind = response_body_kind(status_code, &header_lines, request_allows_response_body);
    let can_reuse = response_allows_reuse(&header_lines)
        && matches!(
            body_kind,
            ResponseBodyKind::None | ResponseBodyKind::ContentLength(_)
        );
    let forwarded_headers = forwarded_headers
        .into_iter()
        .filter(|(name, _)| {
            !(name.as_str().eq_ignore_ascii_case("connection")
                || name.as_str().eq_ignore_ascii_case("keep-alive")
                || (matches!(body_kind, ResponseBodyKind::Chunked)
                    && name.as_str().eq_ignore_ascii_case("transfer-encoding")))
        })
        .collect();

    Ok(ParsedResponseHead {
        status_code,
        body_kind,
        can_reuse,
        forwarded_headers,
    })
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
        Full::new(Bytes::from(body)).map_err(|never| match never {}),
        connection,
    )
    .boxed()
}

fn response_body_kind(
    status_code: u16,
    header_lines: &[&str],
    request_allows_response_body: bool,
) -> ResponseBodyKind {
    if !request_allows_response_body
        || (100..200).contains(&status_code)
        || status_code == 204
        || status_code == 304
    {
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
        if buffer.len() > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response header too large",
            ));
        }
        if let Some(index) = find_header_end(buffer) {
            return Ok(index);
        }
    }
}

enum ChunkedState {
    SizeLine(Vec<u8>),
    Data(usize),
    DataCrLf(usize),
    Trailers(Vec<u8>),
    Done,
}

fn split_host_port(target: &str, default_port: u16) -> Result<(&str, u16), String> {
    if let Some(rest) = target.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| "invalid ipv6 host".to_string())?;
        let host = &rest[..end];
        let suffix = &rest[end + 1..];
        if suffix.is_empty() {
            return Ok((host, default_port));
        }
        let port = suffix
            .strip_prefix(':')
            .ok_or_else(|| "invalid ipv6 host".to_string())?
            .parse()
            .map_err(|_| "invalid port".to_string())?;
        return Ok((host, port));
    }
    if let Some((host, port)) = target.rsplit_once(':') {
        let port = port.parse().map_err(|_| "invalid port".to_string())?;
        Ok((host, port))
    } else {
        Ok((target, default_port))
    }
}

#[cfg(feature = "test-utils")]
mod tests {
    // Imports not provided by `use super::*` (these were formerly cfg-gated at the top level)
    use crate::runtime::RuntimeSnapshot;
    use std::future::Future;
    use std::net::SocketAddr;
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone)]
    pub struct HttpProxyTestResult {
        pub body: String,
        pub account_name: String,
        pub used_pool_selection: bool,
    }

    #[derive(Debug, Clone)]
    pub struct HttpBodyTestResult {
        pub body: String,
    }

    #[derive(Debug, Clone)]
    pub struct ReusedUpstreamTestResult {
        pub body: String,
        pub upstream_accepts: usize,
    }

    #[derive(Debug, Clone)]
    pub struct StreamingResponseTestResult {
        pub first_chunk_latency: Duration,
        pub first_chunk: String,
        pub full_body: String,
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

    #[derive(Debug, Clone)]
    pub struct HttpStatusBodyTestResult {
        pub status_code: u16,
        pub body: String,
    }

    #[derive(Debug, Clone)]
    pub struct TimeoutTestResult {
        pub elapsed: Duration,
    }

    #[derive(Debug, Clone)]
    pub struct LiveFailureRecoveryTestResult {
        pub status_code: u16,
        pub state_summary: String,
        pub selectable_after_failure: bool,
        pub recovered_account: String,
    }

    #[derive(Debug, Clone)]
    pub struct LiveFailureLatencyTestResult {
        pub status_code: u16,
        pub elapsed: Duration,
    }

    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

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

    pub async fn proxy_http_origin_form_ipv6_for_test() -> Result<HttpBodyTestResult, String> {
        let upstream = spawn_http_upstream().await;
        let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
        let addr = spawn_test_proxy(pool, move |_account_name, host, _port| async move {
            if host != "::1" {
                return Err(io::Error::other(format!("unexpected ipv6 host {host}")));
            }
            TcpStream::connect(upstream).await
        })
        .await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(b"GET /health HTTP/1.1\r\nHost: [::1]\r\nConnection: close\r\n\r\n")
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

    pub async fn proxy_http_body_completes_for_keep_alive_upstream_for_test()
    -> Result<HttpBodyTestResult, String> {
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
                b"GET http://intranet.zju.edu.cn/index.html HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
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

    pub async fn proxy_http_reuses_upstream_connection_for_test()
    -> Result<ReusedUpstreamTestResult, String> {
        let (upstream, accepts) = spawn_reusable_keep_alive_http_upstream().await;
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
                b"GET http://intranet.zju.edu.cn/first HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: keep-alive\r\n\r\nGET http://intranet.zju.edu.cn/second HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        let response = String::from_utf8(response).map_err(|err| err.to_string())?;
        let body = extract_first_response_body(&response)?.to_string();
        Ok(ReusedUpstreamTestResult {
            body,
            upstream_accepts: accepts.load(std::sync::atomic::Ordering::SeqCst),
        })
    }

    pub async fn proxy_http_reuses_upstream_connection_after_head_response_for_test()
    -> Result<ReusedUpstreamTestResult, String> {
        let (upstream, accepts) = spawn_reusable_keep_alive_head_http_upstream().await;
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
                b"HEAD http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: keep-alive\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let first_status = tokio::time::timeout(
            Duration::from_millis(200),
            read_http_head_response_status(&mut client),
        )
        .await
        .map_err(|_| "proxy did not complete keep-alive HEAD response in time".to_string())??;
        if first_status != 200 {
            return Err(format!("unexpected HEAD status code: {first_status}"));
        }

        client
            .write_all(
                b"GET http://intranet.zju.edu.cn/second HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let (second_status, body) = read_http_response_status_and_body(&mut client).await?;
        if second_status != 200 {
            return Err(format!("unexpected second status code: {second_status}"));
        }

        Ok(ReusedUpstreamTestResult {
            body,
            upstream_accepts: accepts.load(std::sync::atomic::Ordering::SeqCst),
        })
    }

    pub async fn proxy_http_cached_vpn_reuse_failure_recovers_live_session_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        proxy_http_cached_vpn_reuse_failure_recovers_live_session_with_second_request_connection_for_test(
            "close",
        )
        .await
    }

    pub async fn proxy_http_cached_vpn_reuse_failure_recovers_live_session_with_keep_alive_request_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        proxy_http_cached_vpn_reuse_failure_recovers_live_session_with_second_request_connection_for_test(
            "keep-alive",
        )
        .await
    }

    async fn proxy_http_cached_vpn_reuse_failure_recovers_live_session_with_second_request_connection_for_test(
        second_request_connection: &str,
    ) -> Result<LiveFailureRecoveryTestResult, String> {
        let upstream = spawn_keep_alive_http_upstream_then_close().await;
        let transport = smelly_connect::transport::TransportStack::new(move |_| async move {
            let stream = TcpStream::connect(upstream).await?;
            Ok(smelly_connect::transport::VpnStream::new(stream))
        })
        .with_icmp_pinger(|_| async {
            Err(io::Error::other("forced cached upstream probe failure"))
        });
        let session =
            smelly_connect::test_support::session::session_with_runtime_resources_and_transport(
                "libdb.zju.edu.cn",
                std::net::Ipv4Addr::new(10, 0, 0, 8),
                transport,
            );
        let pool = SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;
        let addr = spawn_single_live_client_proxy(pool.clone(), DEFAULT_CONNECT_TIMEOUT).await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                b"GET http://libdb.zju.edu.cn/first HTTP/1.1\r\nHost: libdb.zju.edu.cn\r\nConnection: keep-alive\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let first_status = read_http_response_status_and_consume(&mut client).await?;
        if first_status != 200 {
            return Err(format!("unexpected first status code: {first_status}"));
        }
        client
            .write_all(
                format!(
                    "GET http://libdb.zju.edu.cn/second HTTP/1.1\r\nHost: libdb.zju.edu.cn\r\nConnection: {second_request_connection}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .map_err(|err| err.to_string())?;
        let second_status = read_http_response_status_to_end(&mut client).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(LiveFailureRecoveryTestResult {
            status_code: second_status,
            state_summary: pool.state_summary_for_test().await,
            selectable_after_failure: pool.has_selectable_nodes_for_test().await,
            recovered_account: "acct-01".to_string(),
        })
    }

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

    pub async fn proxy_http_streams_chunked_request_body_for_test()
    -> Result<HttpBodyTestResult, String> {
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

    pub async fn proxy_http_strips_proxy_authorization_for_test()
    -> Result<HttpBodyTestResult, String> {
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

    pub async fn proxy_http_preserves_non_utf8_request_header_bytes_for_test()
    -> Result<HttpBodyTestResult, String> {
        let upstream = spawn_non_utf8_request_header_capture_upstream().await;
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
                b"GET http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nX-Test: \x80\xffbin\r\nConnection: close\r\n\r\n",
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

    pub async fn proxy_http_preserves_non_utf8_response_header_bytes_for_test() -> Result<(), String>
    {
        let upstream = spawn_non_utf8_response_header_upstream().await;
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
                b"GET http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;

        let expected_header = b"x-test: \x80\xffbin\r\n";
        if !response
            .windows(expected_header.len())
            .any(|window| window == expected_header)
        {
            return Err(format!(
                "proxy rewrote upstream header bytes: {:?}",
                response
            ));
        }
        Ok(())
    }

    pub async fn proxy_http_streams_response_body_for_test()
    -> Result<StreamingResponseTestResult, String> {
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

    pub async fn proxy_http_head_response_for_test() -> Result<HttpStatusBodyTestResult, String> {
        let upstream = spawn_head_response_upstream().await;
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
                b"HEAD http://intranet.zju.edu.cn/health HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
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
        let body = response
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .to_string();

        Ok(HttpStatusBodyTestResult { status_code, body })
    }

    pub async fn proxy_http_rejects_oversized_response_headers_for_test()
    -> Result<NoReadySessionResult, String> {
        let upstream = spawn_oversized_header_upstream().await;
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

    pub async fn proxy_http_direct_forward_for_test() -> Result<HttpBodyTestResult, String> {
        let upstream = spawn_http_upstream().await;
        let session =
            unmatched_live_session_for_test("example.test", std::net::Ipv4Addr::LOCALHOST);
        let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
        let addr = spawn_single_live_client_proxy(pool, DEFAULT_CONNECT_TIMEOUT).await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                format!(
                    "GET http://example.test:{}/health HTTP/1.1\r\nHost: example.test:{}\r\nConnection: close\r\n\r\n",
                    upstream.port(),
                    upstream.port()
                )
                .as_bytes(),
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

    pub async fn proxy_http_live_vpn_forward_for_test() -> Result<HttpBodyTestResult, String> {
        let upstream = spawn_http_upstream().await;
        let session = smelly_connect::test_support::session::session_with_domain_match(
            "vpn.test",
            std::net::Ipv4Addr::LOCALHOST,
        );
        let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
        let addr = spawn_single_live_client_proxy(pool, DEFAULT_CONNECT_TIMEOUT).await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                format!(
                    "GET http://vpn.test:{}/health HTTP/1.1\r\nHost: vpn.test:{}\r\nConnection: close\r\n\r\n",
                    upstream.port(),
                    upstream.port()
                )
                .as_bytes(),
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

    pub async fn proxy_connect_direct_for_test() -> Result<ConnectProxyTestResult, String> {
        let upstream = spawn_echo_upstream().await;
        let session =
            unmatched_live_session_for_test("example.test", std::net::Ipv4Addr::LOCALHOST);
        let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
        let addr = spawn_single_live_client_proxy(pool, DEFAULT_CONNECT_TIMEOUT).await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                format!(
                    "CONNECT example.test:{} HTTP/1.1\r\nHost: example.test:{}\r\nConnection: close\r\n\r\n",
                    upstream.port(),
                    upstream.port()
                )
                .as_bytes(),
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
        Ok(ConnectProxyTestResult {
            account_name: "acct-01".to_string(),
            echoed_bytes: echoed.to_vec(),
        })
    }

    pub async fn proxy_http_live_vpn_connect_for_test() -> Result<ConnectProxyTestResult, String> {
        let upstream = spawn_echo_upstream().await;
        let session = smelly_connect::test_support::session::session_with_domain_match(
            "vpn.test",
            std::net::Ipv4Addr::LOCALHOST,
        );
        let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
        let addr = spawn_single_live_client_proxy(pool, DEFAULT_CONNECT_TIMEOUT).await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                format!(
                    "CONNECT vpn.test:{} HTTP/1.1\r\nHost: vpn.test:{}\r\nConnection: close\r\n\r\n",
                    upstream.port(),
                    upstream.port()
                )
                .as_bytes(),
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
        Ok(ConnectProxyTestResult {
            account_name: "acct-01".to_string(),
            echoed_bytes: echoed.to_vec(),
        })
    }

    pub async fn proxy_http_direct_failure_does_not_open_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        let blocker = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|err| err.to_string())?;
        let blocked_port = blocker.local_addr().map_err(|err| err.to_string())?.port();
        drop(blocker);

        let session =
            unmatched_live_session_for_test("example.test", std::net::Ipv4Addr::LOCALHOST);
        let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
        let addr = spawn_single_live_client_proxy(pool.clone(), DEFAULT_CONNECT_TIMEOUT).await?;

        let status = request_connect_status_for_target(addr, "example.test", blocked_port).await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(LiveFailureRecoveryTestResult {
            status_code: status.status_code,
            state_summary: pool.state_summary_for_test().await,
            selectable_after_failure: pool.has_selectable_nodes_for_test().await,
            recovered_account: "acct-01".to_string(),
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

    pub async fn proxy_http_connect_failure_runtime_status_for_test()
    -> Result<RuntimeSnapshot, String> {
        let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
        let stats = RuntimeStats::default();
        let addr = spawn_test_proxy_with_stats(
            pool.clone(),
            stats.clone(),
            |_account_name, _host, _port| async move { Err(io::Error::other("upstream failed")) },
        )
        .await?;

        let _ = request_connect_status(addr).await?;
        Ok(stats.snapshot(pool.summary().await))
    }

    pub async fn proxy_http_cached_reuse_success_preserves_runtime_status_for_test()
    -> Result<(RuntimeSnapshot, usize), String> {
        let (upstream, accepts) = spawn_reusable_keep_alive_http_upstream().await;
        let transport = smelly_connect::transport::TransportStack::new(move |_| async move {
            let stream = TcpStream::connect(upstream).await?;
            Ok(smelly_connect::transport::VpnStream::new(stream))
        });
        let session =
            smelly_connect::test_support::session::session_with_runtime_resources_and_transport(
                "intranet.zju.edu.cn",
                std::net::Ipv4Addr::new(10, 0, 0, 8),
                transport,
            );
        let pool = SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;
        let stats = RuntimeStats::default();
        let addr = spawn_single_live_client_proxy_with_stats(
            pool.clone(),
            stats.clone(),
            DEFAULT_CONNECT_TIMEOUT,
        )
        .await?;

        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                b"GET http://intranet.zju.edu.cn/first HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: keep-alive\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let first_status = read_http_response_status_and_consume(&mut client).await?;
        if first_status != 200 {
            return Err(format!("unexpected first status code: {first_status}"));
        }

        stats.record_connect_failure();

        client
            .write_all(
                b"GET http://intranet.zju.edu.cn/second HTTP/1.1\r\nHost: intranet.zju.edu.cn\r\nConnection: close\r\n\r\n",
            )
            .await
            .map_err(|err| err.to_string())?;
        let second_status = read_http_response_status_to_end(&mut client).await?;
        if second_status != 200 {
            return Err(format!("unexpected second status code: {second_status}"));
        }

        Ok((
            stats.snapshot(pool.summary().await),
            accepts.load(std::sync::atomic::Ordering::SeqCst),
        ))
    }

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

    pub async fn proxy_http_live_connect_failure_recovery_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        let session = smelly_connect::test_support::session::session_with_failing_domain_match(
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
            let _ = handle_live_client(
                stream,
                serve_pool,
                RuntimeStats::default(),
                DEFAULT_CONNECT_TIMEOUT,
            )
            .await;
        });

        let status = request_connect_status(addr).await?;
        let state_summary = pool.state_summary_for_test().await;
        let selectable_after_failure = pool.has_selectable_nodes_for_test().await;
        Ok(LiveFailureRecoveryTestResult {
            status_code: status.status_code,
            state_summary,
            selectable_after_failure,
            recovered_account: "acct-01".to_string(),
        })
    }

    pub async fn proxy_http_live_connect_failure_does_not_wait_for_probe_for_test()
    -> Result<LiveFailureLatencyTestResult, String> {
        let probe_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let session =
            smelly_connect::test_support::session::session_with_failing_domain_match_and_delayed_icmp(
                "libdb.zju.edu.cn",
                std::net::Ipv4Addr::new(10, 0, 0, 8),
                Duration::from_millis(50),
                probe_count,
            );
        let pool = SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;
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

        let started = Instant::now();
        let status = request_connect_status(addr).await?;
        Ok(LiveFailureLatencyTestResult {
            status_code: status.status_code,
            elapsed: started.elapsed(),
        })
    }

    pub async fn proxy_http_route_rejection_does_not_open_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        let session =
            unmatched_live_session_for_test("example.test", std::net::Ipv4Addr::LOCALHOST);
        let pool = SessionPool::from_live_sessions_with_route_policy_for_test(
            vec![("acct-01", session)],
            smelly_connect::domain::route_policy::RoutePolicy::block_non_resource_targets(),
        )
        .await;
        let addr = spawn_single_live_client_proxy(pool.clone(), DEFAULT_CONNECT_TIMEOUT).await?;

        let status_code = request_connect_status_for_target(addr, "example.test", 443)
            .await?
            .status_code;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(LiveFailureRecoveryTestResult {
            status_code,
            state_summary: pool.state_summary_for_test().await,
            selectable_after_failure: pool.has_selectable_nodes_for_test().await,
            recovered_account: "acct-01".to_string(),
        })
    }

    pub async fn proxy_http_timeout_does_not_open_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        let session = smelly_connect::test_support::session::session_with_slow_domain_match(
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

    pub async fn proxy_http_immediate_timeout_status_for_test()
    -> Result<NoReadySessionResult, String> {
        let session =
            smelly_connect::test_support::session::session_with_immediate_timeout_domain_match(
                "jwxt.sit.edu.cn",
                std::net::Ipv4Addr::new(10, 0, 0, 8),
            );
        let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|err| err.to_string())?;
        let addr = listener.local_addr().map_err(|err| err.to_string())?;
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let _ = handle_live_client(
                stream,
                pool,
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
        Ok(NoReadySessionResult { status_code })
    }

    pub async fn proxy_http_allow_all_failure_does_not_open_for_test()
    -> Result<LiveFailureRecoveryTestResult, String> {
        let session =
            unmatched_live_session_for_test("example.test", std::net::Ipv4Addr::LOCALHOST)
                .with_allow_all_routes(true);
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

        let status_code = request_connect_status_for_target(addr, "example.test", 443)
            .await?
            .status_code;
        Ok(LiveFailureRecoveryTestResult {
            status_code,
            state_summary: pool.state_summary_for_test().await,
            selectable_after_failure: pool.has_selectable_nodes_for_test().await,
            recovered_account: "acct-01".to_string(),
        })
    }

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
            if let Err(err) = handle_live_client(stream, pool, stats, DEFAULT_CONNECT_TIMEOUT).await
            {
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

    pub async fn proxy_http_over_capacity_for_test() -> Result<NoReadySessionResult, String> {
        let upstream = spawn_http_upstream().await;
        let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
        let addr =
            spawn_test_proxy_with_limit(pool, 1, move |_account_name, _host, _port| async move {
                TcpStream::connect(upstream).await
            })
            .await?;

        let blocker = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let result = request_no_ready_session(addr).await;
        drop(blocker);
        result
    }

    async fn spawn_test_proxy<F, Fut>(pool: SessionPool, connector: F) -> Result<SocketAddr, String>
    where
        F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
    {
        spawn_test_proxy_internal(
            pool,
            None,
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_MAX_IN_FLIGHT_CONNECTIONS,
            connector,
        )
        .await
    }

    async fn spawn_test_proxy_with_stats<F, Fut>(
        pool: SessionPool,
        stats: RuntimeStats,
        connector: F,
    ) -> Result<SocketAddr, String>
    where
        F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
    {
        spawn_test_proxy_internal(
            pool,
            Some(stats),
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_MAX_IN_FLIGHT_CONNECTIONS,
            connector,
        )
        .await
    }

    async fn spawn_test_proxy_with_timeout<F, Fut>(
        pool: SessionPool,
        connect_timeout: Duration,
        connector: F,
    ) -> Result<SocketAddr, String>
    where
        F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
    {
        spawn_test_proxy_internal(
            pool,
            None,
            connect_timeout,
            DEFAULT_MAX_IN_FLIGHT_CONNECTIONS,
            connector,
        )
        .await
    }

    async fn spawn_test_proxy_with_limit<F, Fut>(
        pool: SessionPool,
        max_in_flight_connections: usize,
        connector: F,
    ) -> Result<SocketAddr, String>
    where
        F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
    {
        spawn_test_proxy_internal(
            pool,
            None,
            DEFAULT_CONNECT_TIMEOUT,
            max_in_flight_connections,
            connector,
        )
        .await
    }

    async fn spawn_test_proxy_internal<F, Fut>(
        pool: SessionPool,
        stats: Option<RuntimeStats>,
        connect_timeout: Duration,
        max_in_flight_connections: usize,
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
        let limiter = Arc::new(Semaphore::new(max_in_flight_connections));
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let permit = limiter.clone().try_acquire_owned();
                let pool = pool.clone();
                let stats = stats.clone();
                let connector = connector.clone();
                let connect_timeout = connect_timeout;
                match permit {
                    Ok(permit) => {
                        tokio::spawn(async move {
                            let _permit = permit;
                            let _ = handle_client(stream, pool, stats, connect_timeout, connector)
                                .await;
                        });
                    }
                    Err(_) => {
                        tokio::spawn(async move {
                            let _ = reject_over_capacity_http(stream).await;
                        });
                    }
                }
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

    async fn request_connect_status(addr: SocketAddr) -> Result<NoReadySessionResult, String> {
        request_connect_status_for_target(addr, "libdb.zju.edu.cn", 443).await
    }

    async fn request_connect_status_for_target(
        addr: SocketAddr,
        host: &str,
        port: u16,
    ) -> Result<NoReadySessionResult, String> {
        let mut client = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        client
            .write_all(
                format!(
                    "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .map_err(|err| err.to_string())?;
        let mut response = [0_u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut response))
            .await
            .map_err(|_| "timed out waiting for connect response".to_string())?
            .map_err(|err| err.to_string())?;
        let response = String::from_utf8(response[..n].to_vec()).map_err(|err| err.to_string())?;
        let status_line = response.lines().next().unwrap_or_default().to_string();
        let status_code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| format!("invalid status line: {status_line}"))?;
        Ok(NoReadySessionResult { status_code })
    }

    async fn spawn_single_live_client_proxy(
        pool: SessionPool,
        connect_timeout: Duration,
    ) -> Result<SocketAddr, String> {
        spawn_single_live_client_proxy_with_stats(pool, RuntimeStats::default(), connect_timeout)
            .await
    }

    async fn spawn_single_live_client_proxy_with_stats(
        pool: SessionPool,
        stats: RuntimeStats,
        connect_timeout: Duration,
    ) -> Result<SocketAddr, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|err| err.to_string())?;
        let addr = listener.local_addr().map_err(|err| err.to_string())?;
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let _ = handle_live_client(stream, pool, stats, connect_timeout).await;
        });
        Ok(addr)
    }

    fn unmatched_live_session_for_test(
        host: &str,
        ip: std::net::Ipv4Addr,
    ) -> smelly_connect::session::EasyConnectSession {
        let mut system_dns = std::collections::HashMap::new();
        system_dns.insert(host.to_string(), std::net::IpAddr::V4(ip));
        smelly_connect::session::EasyConnectSession::new(
            "10.0.0.8".parse().unwrap(),
            smelly_connect::resource::ResourceSet::default(),
            smelly_connect::resolver::SessionResolver::new(
                std::collections::HashMap::new(),
                None,
                system_dns,
            ),
            smelly_connect::session::EasyConnectSession::failing_transport(
                "direct route should bypass vpn transport",
            ),
        )
    }

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
        let upstream_cache = Arc::new(tokio::sync::Mutex::new(None::<CachedUpstream<TcpStream>>));
        let io = TokioIo::new(client);
        hyper_server_http1::Builder::new()
            .half_close(true)
            .serve_connection(
                io,
                service_fn(move |request| {
                    let pool = pool.clone();
                    let stats = stats.clone();
                    let connector = connector.clone();
                    let upstream_cache = Arc::clone(&upstream_cache);
                    async move {
                        Ok::<_, Infallible>(
                            handle_test_request(
                                request,
                                pool,
                                stats,
                                connect_timeout,
                                connector,
                                upstream_cache,
                            )
                            .await,
                        )
                    }
                }),
            )
            .with_upgrades()
            .await
            .map_err(|err| err.to_string())
    }

    async fn handle_test_request<F, Fut>(
        request: Request<Incoming>,
        pool: SessionPool,
        stats: Option<RuntimeStats>,
        connect_timeout: Duration,
        connector: F,
        upstream_cache: Arc<tokio::sync::Mutex<Option<CachedUpstream<TcpStream>>>>,
    ) -> Response<ProxyBody>
    where
        F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
    {
        let request_id = next_request_id();
        let account_name = match pool.next_account_name().await {
            Ok(name) => name,
            Err(_) => {
                log_no_ready_session(request_id, "http");
                if let Some(stats) = &stats {
                    stats.record_service_unavailable_no_ready_session(ProxyProtocol::Http);
                }
                return empty_response(StatusCode::SERVICE_UNAVAILABLE);
            }
        };

        if request.method() == Method::CONNECT {
            let (host, port, target) = match resolve_connect_target(&request) {
                Ok(target) => target,
                Err(_) => return empty_response(StatusCode::BAD_REQUEST),
            };
            log_request_accepted(
                Some(request_id),
                "connect",
                &target,
                LiveRouteBackend::Vpn,
                &account_name,
            );
            let on_upgrade = upgrade::on(request);
            let connect_started = Instant::now();
            log_upstream_connect_start(
                request_id,
                "connect",
                LiveRouteBackend::Vpn,
                Some(&account_name),
                &target,
                connect_timeout,
            );
            let upstream = connector(account_name, host, port);
            let upstream = match connect_with_timeout(connect_timeout, upstream).await {
                Ok(upstream) => {
                    log_upstream_connect_success(request_id, "connect", &target, connect_started);
                    upstream
                }
                Err(err) => {
                    log_upstream_connect_failure(
                        request_id,
                        "connect",
                        &target,
                        connect_started,
                        &err,
                    );
                    if let Some(stats) = &stats {
                        stats.record_connect_failure();
                    }
                    return gateway_error_response(&err);
                }
            };
            let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Http));
            tokio::spawn(async move {
                let Ok(upgraded) = on_upgrade.await else {
                    tracing::warn!(request_id, target = %target, "http connect upgrade failed");
                    return;
                };
                tracing::info!(request_id, target = %target, "http connect tunnel established");
                let mut client = TokioIo::new(upgraded);
                let mut upstream = upstream;
                let relay_started = Instant::now();
                match relay_upgraded_tunnel(&mut client, &mut upstream, connection.as_ref()).await {
                    Ok((client_to_upstream_bytes, upstream_to_client_bytes)) => {
                        tracing::info!(
                            request_id,
                            target = %target,
                            elapsed_ms = elapsed_ms(relay_started),
                            client_to_upstream_bytes,
                            upstream_to_client_bytes,
                            "http connect tunnel relay finished"
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            request_id,
                            target = %target,
                            elapsed_ms = elapsed_ms(relay_started),
                            error = %err,
                            "http connect tunnel relay failed"
                        );
                    }
                }
            });
            return connect_established_response();
        }

        let (host, port, target, uri) = match resolve_forward_target(&request) {
            Ok(target) => target,
            Err(_) => return empty_response(StatusCode::BAD_REQUEST),
        };
        log_request_accepted(
            Some(request_id),
            "http",
            &target,
            LiveRouteBackend::Vpn,
            &account_name,
        );

        let wants_keep_alive = client_requests_keep_alive(&request);
        let upstream = take_cached_upstream(&upstream_cache, &host, port).await;
        let upstream = match upstream {
            Some(upstream) => Ok(upstream.stream),
            None => {
                let connect_started = Instant::now();
                log_upstream_connect_start(
                    request_id,
                    "http",
                    LiveRouteBackend::Vpn,
                    Some(&account_name),
                    &target,
                    connect_timeout,
                );
                match connect_with_timeout(
                    connect_timeout,
                    connector(account_name, host.clone(), port),
                )
                .await
                {
                    Ok(upstream) => {
                        log_upstream_connect_success(request_id, "http", &target, connect_started);
                        Ok(upstream)
                    }
                    Err(err) => {
                        log_upstream_connect_failure(
                            request_id,
                            "http",
                            &target,
                            connect_started,
                            &err,
                        );
                        Err(err)
                    }
                }
            }
        };
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(err) => {
                if let Some(stats) = &stats {
                    stats.record_connect_failure();
                }
                return gateway_error_response(&err);
            }
        };
        let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Http));
        if wants_keep_alive {
            let (response, reusable, _reuse_error) =
                forward_request_with_reuse(request, uri, upstream, connection).await;
            if let Some(reusable) = reusable {
                store_cached_upstream(
                    &upstream_cache,
                    CachedUpstream {
                        host,
                        port,
                        stream: reusable,
                        metadata: (),
                    },
                )
                .await;
            }
            response
        } else {
            let (response, _forward_error) =
                forward_request(request, uri, upstream, connection).await;
            response
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

    async fn spawn_keep_alive_http_upstream_then_close() -> SocketAddr {
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
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                )
                .await
                .unwrap();
            let _ = socket.shutdown().await;
        });
        addr
    }

    async fn spawn_reusable_keep_alive_http_upstream()
    -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepts_task = Arc::clone(&accepts);
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            accepts_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            for _ in 0..2 {
                let mut buf = [0_u8; 1024];
                let _ = socket.read(&mut buf).await.unwrap();
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            }
        });
        (addr, accepts)
    }

    async fn spawn_reusable_keep_alive_head_http_upstream()
    -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepts_task = Arc::clone(&accepts);
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            accepts_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            let mut chunk = [0_u8; 1024];
            let mut first_request = Vec::new();
            loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    return;
                }
                first_request.extend_from_slice(&chunk[..n]);
                if find_header_end(&first_request).is_some() {
                    break;
                }
            }

            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: keep-alive\r\n\r\n",
                )
                .await
                .unwrap();

            let mut second_request = Vec::new();
            loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    return;
                }
                second_request.extend_from_slice(&chunk[..n]);
                if find_header_end(&second_request).is_some() {
                    break;
                }
            }

            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        (addr, accepts)
    }

    fn extract_first_response_body(response: &str) -> Result<&str, String> {
        let (headers, rest) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| "missing response header terminator".to_string())?;
        let status_line = headers.lines().next().unwrap_or_default();
        if !status_line.starts_with("HTTP/1.1 200") {
            return Err(format!("unexpected status line: {status_line}"));
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
            })
            .ok_or_else(|| "missing content-length".to_string())?;
        if rest.len() < content_length {
            return Err("response body shorter than content-length".to_string());
        }
        Ok(&rest[..content_length])
    }

    async fn read_http_response_status_and_consume(stream: &mut TcpStream) -> Result<u16, String> {
        let (status_code, _body) = read_http_response_status_and_body(stream).await?;
        Ok(status_code)
    }

    async fn read_http_head_response_status(stream: &mut TcpStream) -> Result<u16, String> {
        let mut buffer = Vec::new();
        let header_end = read_headers(stream, &mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        let head = parse_upstream_response_head(&buffer[..header_end], false)?;
        if !matches!(head.body_kind, ResponseBodyKind::None) {
            return Err("expected HEAD response without body".to_string());
        }
        if buffer.len() != header_end {
            return Err("unexpected body bytes after HEAD response headers".to_string());
        }
        Ok(head.status_code)
    }

    async fn read_http_response_status_and_body(
        stream: &mut TcpStream,
    ) -> Result<(u16, String), String> {
        let mut buffer = Vec::new();
        let header_end = read_headers(stream, &mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        let head = parse_upstream_response_head(&buffer[..header_end], true)?;
        let initial_body_len = buffer.len().saturating_sub(header_end);
        let ResponseBodyKind::ContentLength(length) = head.body_kind else {
            return Err("expected content-length response for keep-alive test".to_string());
        };
        let mut body = buffer[header_end..].to_vec();
        let mut remaining = length.saturating_sub(initial_body_len);
        let mut chunk = [0_u8; 1024];
        while remaining > 0 {
            let limit = remaining.min(chunk.len());
            let n = stream
                .read(&mut chunk[..limit])
                .await
                .map_err(|err| err.to_string())?;
            if n == 0 {
                return Err("response body closed early".to_string());
            }
            body.extend_from_slice(&chunk[..n]);
            remaining -= n;
        }
        body.truncate(length);
        let body = String::from_utf8(body).map_err(|err| err.to_string())?;
        Ok((head.status_code, body))
    }

    async fn read_http_response_status_to_end(stream: &mut TcpStream) -> Result<u16, String> {
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        let response = String::from_utf8(response).map_err(|err| err.to_string())?;
        let status_line = response.lines().next().unwrap_or_default().to_string();
        status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| format!("invalid status line: {status_line}"))
    }

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
                let n =
                    match tokio::time::timeout(Duration::from_millis(200), socket.read(&mut chunk))
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

    async fn spawn_chunked_request_body_echo_upstream() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];

            loop {
                let n =
                    match tokio::time::timeout(Duration::from_millis(200), socket.read(&mut chunk))
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

    async fn spawn_non_utf8_request_header_capture_upstream() -> SocketAddr {
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
            let expected_header = b"x-test: \x80\xffbin\r\n";
            let body = if request
                .windows(expected_header.len())
                .any(|window| window == expected_header)
            {
                "preserved"
            } else {
                "rewritten"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        addr
    }

    async fn spawn_non_utf8_response_header_upstream() -> SocketAddr {
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
                .write_all(
                    b"HTTP/1.1 200 OK\r\nX-Test: \x80\xffbin\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .await
                .unwrap();
        });
        addr
    }

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
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello",
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(250)).await;
            socket.write_all(b" world").await.unwrap();
        });
        addr
    }

    async fn spawn_head_response_upstream() -> SocketAddr {
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
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        addr
    }

    async fn spawn_oversized_header_upstream() -> SocketAddr {
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
            let oversized = "a".repeat(17 * 1024);
            let response = format!(
                "HTTP/1.1 200 OK\r\nX-Oversized: {oversized}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        addr
    }

    fn chunked_request_complete(request: &[u8]) -> bool {
        let Some(header_end) = find_header_end(request) else {
            return false;
        };
        let body = &request[header_end..];
        chunked_wire_complete(body)
    }

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
            let Ok(size) =
                usize::from_str_radix(size_line.split(';').next().unwrap_or_default().trim(), 16)
            else {
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
}
