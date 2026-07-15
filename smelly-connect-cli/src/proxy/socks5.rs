use std::io;
use std::net::SocketAddr as StdSocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fast_socks5::server::Socks5ServerProtocol;
use fast_socks5::{ReplyError, Socks5Command};
use fast_socks5::{new_udp_header, parse_udp_request};
use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::pool::SessionPool;
use crate::runtime::{ConnectionGuard, ProxyProtocol, RuntimeStats};

use super::common::{
    LISTENER_ACCEPT_RETRY_BACKOFF, ListenerAcceptRetryLogState, LiveRouteBackend,
    UpstreamConnectError, connect_planned_live_upstream_with_timeout, log_request_accepted,
    plan_live_upstream_connect, should_retry_listener_accept,
};

const DEFAULT_MAX_IN_FLIGHT_CONNECTIONS: usize = 1024;

fn reply_success_addr() -> std::net::SocketAddr {
    std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0))
}

pub async fn serve_socks5_with_shutdown(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    udp_associate_idle_timeout: Option<Duration>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    serve_socks5_with_limit(
        listen,
        pool,
        stats,
        connect_timeout,
        udp_associate_idle_timeout,
        DEFAULT_MAX_IN_FLIGHT_CONNECTIONS,
        shutdown,
    )
    .await
}

async fn serve_socks5_with_limit(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    udp_associate_idle_timeout: Option<Duration>,
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
        protocol = tracing::field::display("socks5"),
        listen = %local_addr,
        "socks5 proxy listening"
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
                        protocol = tracing::field::display("socks5"),
                        error = %err,
                        "socks5 connection task failed"
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
                                protocol = tracing::field::display("socks5"),
                                error = %err,
                                backoff_ms = LISTENER_ACCEPT_RETRY_BACKOFF.as_millis() as u64,
                                "socks5 listener accept hit transient fd exhaustion; retrying"
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
                            if let Err(err) = handle_live_client(
                                stream,
                                pool,
                                stats,
                                connect_timeout,
                                udp_associate_idle_timeout,
                            )
                            .await
                            {
                                tracing::warn!(
                                    protocol = tracing::field::display("socks5"),
                                    error = %err,
                                    "live proxy request failed"
                                );
                            }
                        });
                    }
                    Err(_) => {
                        stats.record_service_unavailable_over_capacity(ProxyProtocol::Socks5);
                        clients.spawn(async move {
                            let _ = reject_over_capacity_socks5(stream).await;
                        });
                    }
                }
            }
        }
    }
    while let Some(result) = clients.join_next().await {
        if let Err(err) = result {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                error = %err,
                "socks5 connection task failed during shutdown"
            );
        }
    }
    Ok(())
}

async fn reject_over_capacity_socks5(stream: TcpStream) -> io::Result<()> {
    let mut stream = stream;
    let mut method_header = [0_u8; 2];
    tokio::io::AsyncReadExt::read_exact(&mut stream, &mut method_header).await?;
    let methods_len = method_header[1] as usize;
    let mut methods = vec![0_u8; methods_len];
    tokio::io::AsyncReadExt::read_exact(&mut stream, &mut methods).await?;
    tokio::io::AsyncWriteExt::write_all(&mut stream, &[0x05, 0x00]).await?;

    let mut request_header = [0_u8; 4];
    tokio::io::AsyncReadExt::read_exact(&mut stream, &mut request_header).await?;
    match request_header[3] {
        0x01 => {
            let mut rest = [0_u8; 6];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut rest).await?;
        }
        0x03 => {
            let mut len = [0_u8; 1];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut len).await?;
            let mut rest = vec![0_u8; len[0] as usize + 2];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut rest).await?;
        }
        0x04 => {
            let mut rest = [0_u8; 18];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut rest).await?;
        }
        _ => {}
    }

    tokio::io::AsyncWriteExt::write_all(&mut stream, &[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    Ok(())
}

async fn handle_live_client(
    client: TcpStream,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    udp_associate_idle_timeout: Option<Duration>,
) -> Result<(), String> {
    let (proto, cmd, target_addr) = Socks5ServerProtocol::accept_no_auth(client)
        .await
        .map_err(|err| err.to_string())?
        .read_command()
        .await
        .map_err(|err| err.to_string())?;
    let pooled = match pool.acquire().await {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                "no ready session"
            );
            stats.record_service_unavailable_no_ready_session(ProxyProtocol::Socks5);
            proto
                .reply_error(&ReplyError::NetworkUnreachable)
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
    };
    let account_name = pooled.account_name().to_string();
    let Some(session) = pooled.session().cloned() else {
        tracing::warn!(
            protocol = tracing::field::display("socks5"),
            "acquired session with no inner session"
        );
        stats.record_service_unavailable_no_ready_session(ProxyProtocol::Socks5);
        proto
            .reply_error(&ReplyError::NetworkUnreachable)
            .await
            .map_err(|err| err.to_string())?;
        return Ok(());
    };

    match cmd {
        Socks5Command::TCPConnect => {
            let (host, port) = target_addr.into_string_and_port();
            let target = format!("{host}:{port}");
            let route_plan = match plan_live_upstream_connect(&session, &host, port).await {
                Ok(route_plan) => route_plan,
                Err((err, route_backend)) => {
                    if !matches!(err, UpstreamConnectError::RouteRejected) {
                        stats.record_connect_failure();
                    }
                    if matches!(route_backend, LiveRouteBackend::Vpn)
                        && !matches!(
                            err,
                            UpstreamConnectError::RouteRejected | UpstreamConnectError::TimedOut
                        )
                    {
                        pool.report_failure(&account_name).await;
                    }
                    proto
                        .reply_error(&map_socks5_reply_error(&err))
                        .await
                        .map_err(|reply_err| reply_err.to_string())?;
                    return Ok(());
                }
            };
            let route_backend = route_plan.backend();
            log_request_accepted(None, "socks5", &target, route_backend, &account_name);
            let mut upstream = match connect_planned_live_upstream_with_timeout(
                connect_timeout,
                &session,
                &host,
                port,
                route_plan,
            )
            .await
            {
                Ok((upstream, _route_backend)) => upstream,
                Err((err, route_backend)) => {
                    if !matches!(err, UpstreamConnectError::RouteRejected) {
                        stats.record_connect_failure();
                    }
                    if matches!(route_backend, LiveRouteBackend::Vpn)
                        && !matches!(
                            err,
                            UpstreamConnectError::RouteRejected | UpstreamConnectError::TimedOut
                        )
                    {
                        pool.report_failure(&account_name).await;
                    }
                    proto
                        .reply_error(&map_socks5_reply_error(&err))
                        .await
                        .map_err(|reply_err| reply_err.to_string())?;
                    return Ok(());
                }
            };
            stats.record_connect_success();
            let connection = stats.open_connection(ProxyProtocol::Socks5);
            let mut client = proto
                .reply_success(reply_success_addr())
                .await
                .map_err(|err| err.to_string())?;
            relay_tunnel(&mut client, &mut upstream, Some(&connection)).await
        }
        Socks5Command::UDPAssociate => {
            tracing::info!(
                protocol = tracing::field::display("socks5"),
                target = %target_addr,
                account = %account_name,
                "udp associate accepted"
            );
            stats.record_connect_success();
            let connection = stats.open_connection(ProxyProtocol::Socks5);
            let bind_addr = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|err| err.to_string())?;
            let reply_addr = bind_addr.local_addr().map_err(|err| err.to_string())?;
            let client = proto
                .reply_success(reply_addr)
                .await
                .map_err(|err| err.to_string())?;
            relay_udp_associate(
                client,
                bind_addr,
                session,
                udp_associate_idle_timeout,
                Some(&connection),
            )
            .await
        }
        _ => {
            proto
                .reply_error(&ReplyError::CommandNotSupported)
                .await
                .map_err(|err| err.to_string())?;
            Ok(())
        }
    }
}

async fn relay_tunnel(
    client: &mut TcpStream,
    upstream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    connection: Option<&ConnectionGuard>,
) -> Result<(), String> {
    let (client_to_upstream, upstream_to_client) = copy_bidirectional(client, upstream)
        .await
        .map_err(|err| err.to_string())?;
    if let Some(connection) = connection {
        connection.add_client_to_upstream_bytes(client_to_upstream);
        connection.add_upstream_to_client_bytes(upstream_to_client);
    }
    Ok(())
}

async fn relay_udp_associate(
    mut control: TcpStream,
    inbound: UdpSocket,
    session: smelly_connect::Session,
    udp_associate_idle_timeout: Option<Duration>,
    connection: Option<&ConnectionGuard>,
) -> Result<(), String> {
    let mut client_addr = None::<StdSocketAddr>;
    let mut client_buf = vec![0_u8; 65_536];
    let mut vpn_upstream_buf = vec![0_u8; 65_536];
    let mut direct_upstream_buf = vec![0_u8; 65_536];
    let mut control_buf = [0_u8; 1];
    let mut vpn_outbound = None::<smelly_connect::session::SessionUdpSocket>;
    let mut direct_outbound = None::<UdpSocket>;

    loop {
        tokio::select! {
            _ = async {
                match udp_associate_idle_timeout {
                    Some(timeout) => tokio::time::sleep(timeout).await,
                    None => std::future::pending::<()>().await,
                }
            } => return Ok(()),
            read = tokio::io::AsyncReadExt::read(&mut control, &mut control_buf) => {
                match read.map_err(|err| err.to_string())? {
                    0 => return Ok(()),
                    _ => return Err("unexpected control data on udp associate connection".to_string()),
                }
            }
            recv = inbound.recv_from(&mut client_buf) => {
                let (n, addr) = recv.map_err(|err| err.to_string())?;
                if let Some(expected) = client_addr {
                    if addr != expected {
                        continue;
                    }
                } else {
                    client_addr = Some(addr);
                }

                let (frag, target_addr, data) =
                    parse_udp_request(&client_buf[..n]).await.map_err(|err| err.to_string())?;
                if frag != 0 {
                    continue;
                }

                let (host, port) = target_addr.into_string_and_port();
                let sent = match session.plan_udp_send((host.as_str(), port)).await {
                    Ok(smelly_connect::session::RoutePlan::VpnResolved(_)) => {
                        if vpn_outbound.is_none() {
                            vpn_outbound = Some(session.bind_udp().await.map_err(|err| format!("{err:?}"))?);
                        }
                        match vpn_outbound.as_ref().unwrap().send_to(data, (host.as_str(), port)).await {
                            Ok(sent) => sent,
                            Err(smelly_connect::Error::RouteDecision(_)) => continue,
                            Err(smelly_connect::Error::Resolve(_)) => continue,
                            Err(err) => return Err(format!("{err:?}")),
                        }
                    }
                    Ok(smelly_connect::session::RoutePlan::Direct(addr)) => {
                        if direct_outbound.is_none() {
                            direct_outbound = Some(
                                UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                                    .await
                                    .map_err(|err| err.to_string())?,
                            );
                        }
                        direct_outbound
                            .as_ref()
                            .unwrap()
                            .send_to(data, addr)
                            .await
                            .map_err(|err| err.to_string())?
                    }
                    Err(smelly_connect::Error::RouteDecision(_)) => continue,
                    Err(smelly_connect::Error::Resolve(_)) => continue,
                    Err(err) => return Err(format!("{err:?}")),
                };
                if let Some(connection) = connection {
                    connection.add_client_to_upstream_bytes(sent as u64);
                }
            }
            recv = async {
                match vpn_outbound.as_ref() {
                    Some(outbound) => outbound
                        .recv_from(&mut vpn_upstream_buf)
                        .await
                        .map_err(|err| format!("{err:?}")),
                    None => std::future::pending::<Result<(usize, StdSocketAddr), String>>().await,
                }
            } => {
                let Some(client_addr) = client_addr else {
                    continue;
                };
                let (n, remote_addr) = recv?;
                let mut packet = new_udp_header(remote_addr).map_err(|err| err.to_string())?;
                packet.extend_from_slice(&vpn_upstream_buf[..n]);
                inbound
                    .send_to(&packet, client_addr)
                    .await
                    .map_err(|err| err.to_string())?;
                if let Some(connection) = connection {
                    connection.add_upstream_to_client_bytes(n as u64);
                }
            }
            recv = async {
                match direct_outbound.as_ref() {
                    Some(outbound) => outbound.recv_from(&mut direct_upstream_buf).await.map_err(|err| err.to_string()),
                    None => std::future::pending::<Result<(usize, StdSocketAddr), String>>().await,
                }
            } => {
                let Some(client_addr) = client_addr else {
                    continue;
                };
                let (n, remote_addr) = recv?;
                let mut packet = new_udp_header(remote_addr).map_err(|err| err.to_string())?;
                packet.extend_from_slice(&direct_upstream_buf[..n]);
                inbound
                    .send_to(&packet, client_addr)
                    .await
                    .map_err(|err| err.to_string())?;
                if let Some(connection) = connection {
                    connection.add_upstream_to_client_bytes(n as u64);
                }
            }
        }
    }
}

fn map_socks5_reply_error(err: &UpstreamConnectError) -> ReplyError {
    match err {
        UpstreamConnectError::TimedOut => ReplyError::ConnectionTimeout,
        UpstreamConnectError::RouteRejected => ReplyError::ConnectionNotAllowed,
        UpstreamConnectError::Failed => ReplyError::NetworkUnreachable,
    }
}
