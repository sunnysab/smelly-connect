use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub enum UpstreamConnectError {
    TimedOut,
    Failed,
    RouteRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveRouteBackend {
    Vpn,
    Direct,
}

pub fn live_route_label(route_backend: LiveRouteBackend) -> &'static str {
    match route_backend {
        LiveRouteBackend::Vpn => "vpn",
        LiveRouteBackend::Direct => "direct",
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LiveRoutePlan {
    Vpn,
    Direct(SocketAddr),
}

impl LiveRoutePlan {
    pub const fn backend(self) -> LiveRouteBackend {
        match self {
            Self::Vpn => LiveRouteBackend::Vpn,
            Self::Direct(_) => LiveRouteBackend::Direct,
        }
    }
}

pub fn log_request_accepted(
    request_id: Option<u64>,
    protocol: &'static str,
    target: &str,
    route_backend: LiveRouteBackend,
    account_name: &str,
) {
    match (request_id, route_backend) {
        (Some(request_id), LiveRouteBackend::Vpn) => tracing::info!(
            request_id,
            protocol = tracing::field::display(protocol),
            target = %target,
            route = %live_route_label(route_backend),
            account = %account_name,
            "request accepted"
        ),
        (Some(request_id), LiveRouteBackend::Direct) => tracing::info!(
            request_id,
            protocol = tracing::field::display(protocol),
            target = %target,
            route = %live_route_label(route_backend),
            "request accepted"
        ),
        (None, LiveRouteBackend::Vpn) => tracing::info!(
            protocol = tracing::field::display(protocol),
            target = %target,
            route = %live_route_label(route_backend),
            account = %account_name,
            "request accepted"
        ),
        (None, LiveRouteBackend::Direct) => tracing::info!(
            protocol = tracing::field::display(protocol),
            target = %target,
            route = %live_route_label(route_backend),
            "request accepted"
        ),
    }
}

pub const LISTENER_ACCEPT_RETRY_BACKOFF: Duration = Duration::from_millis(100);
pub const LISTENER_ACCEPT_WARN_INTERVAL: Duration = Duration::from_secs(5);

const ENFILE_ERRNO: i32 = 23;
const EMFILE_ERRNO: i32 = 24;

#[derive(Debug, Default)]
pub struct ListenerAcceptRetryLogState {
    last_warn_at: Option<Instant>,
}

pub fn should_retry_listener_accept(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(EMFILE_ERRNO | ENFILE_ERRNO))
}

impl ListenerAcceptRetryLogState {
    pub fn should_warn(&mut self) -> bool {
        let now = Instant::now();
        match self.last_warn_at {
            Some(last_warn_at)
                if now.duration_since(last_warn_at) < LISTENER_ACCEPT_WARN_INTERVAL =>
            {
                false
            }
            _ => {
                self.last_warn_at = Some(now);
                true
            }
        }
    }

    pub fn reset(&mut self) {
        self.last_warn_at = None;
    }
}

pub async fn connect_with_timeout<T, E, Fut>(
    timeout: Duration,
    fut: Fut,
) -> Result<T, UpstreamConnectError>
where
    E: std::fmt::Debug,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_err)) => Err(UpstreamConnectError::Failed),
        Err(_) => Err(UpstreamConnectError::TimedOut),
    }
}

pub async fn plan_live_upstream_connect(
    session: &smelly_connect::Session,
    host: &str,
    port: u16,
) -> Result<LiveRoutePlan, (UpstreamConnectError, LiveRouteBackend)> {
    let route = match session.plan_tcp_connect((host, port)).await {
        Ok(route) => route,
        Err(smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed,
        )) => {
            return Err((
                UpstreamConnectError::RouteRejected,
                LiveRouteBackend::Direct,
            ));
        }
        Err(smelly_connect::Error::Transport(
            smelly_connect::error::TransportError::ConnectTimedOut,
        )) => return Err((UpstreamConnectError::TimedOut, LiveRouteBackend::Direct)),
        Err(_err) => return Err((UpstreamConnectError::Failed, LiveRouteBackend::Direct)),
    };

    match route {
        smelly_connect::session::RoutePlan::VpnResolved(_) => Ok(LiveRoutePlan::Vpn),
        smelly_connect::session::RoutePlan::Direct(addr) => Ok(LiveRoutePlan::Direct(addr)),
    }
}

pub async fn connect_planned_live_upstream_with_timeout(
    timeout: Duration,
    session: &smelly_connect::Session,
    host: &str,
    port: u16,
    route_plan: LiveRoutePlan,
) -> Result<
    (smelly_connect::transport::VpnStream, LiveRouteBackend),
    (UpstreamConnectError, LiveRouteBackend),
> {
    let route_backend = route_plan.backend();
    match route_plan {
        LiveRoutePlan::Vpn => {
            connect_session_with_timeout(timeout, session.connect_tcp((host, port)))
                .await
                .map(|upstream| (upstream, route_backend))
                .map_err(|err| (err, route_backend))
        }
        LiveRoutePlan::Direct(addr) => connect_with_timeout(timeout, TcpStream::connect(addr))
            .await
            .map(|stream| {
                (
                    smelly_connect::transport::VpnStream::new(stream),
                    route_backend,
                )
            })
            .map_err(|err| (err, route_backend)),
    }
}

pub async fn connect_session_with_timeout<Fut>(
    timeout: Duration,
    fut: Fut,
) -> Result<smelly_connect::transport::VpnStream, UpstreamConnectError>
where
    Fut: std::future::Future<
            Output = Result<smelly_connect::transport::VpnStream, smelly_connect::Error>,
        >,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed,
        ))) => Err(UpstreamConnectError::RouteRejected),
        Ok(Err(smelly_connect::Error::Transport(
            smelly_connect::error::TransportError::ConnectTimedOut,
        ))) => Err(UpstreamConnectError::TimedOut),
        Ok(Err(_err)) => Err(UpstreamConnectError::Failed),
        Err(_) => Err(UpstreamConnectError::TimedOut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_accept_retries_fd_exhaustion_errors() {
        assert!(should_retry_listener_accept(&io::Error::from_raw_os_error(
            EMFILE_ERRNO
        )));
        assert!(should_retry_listener_accept(&io::Error::from_raw_os_error(
            ENFILE_ERRNO
        )));
    }

    #[test]
    fn listener_accept_does_not_retry_unrelated_errors() {
        assert!(!should_retry_listener_accept(&io::Error::other("boom")));
        assert!(!should_retry_listener_accept(&io::Error::from(
            io::ErrorKind::ConnectionAborted
        )));
    }

    #[tokio::test(start_paused = true)]
    async fn listener_accept_retry_warn_state_rate_limits_repeated_warnings() {
        let mut state = ListenerAcceptRetryLogState::default();

        assert!(state.should_warn());
        assert!(!state.should_warn());

        tokio::time::advance(LISTENER_ACCEPT_WARN_INTERVAL).await;

        assert!(state.should_warn());
    }

    #[test]
    fn listener_accept_retry_warn_state_resets_after_success() {
        let mut state = ListenerAcceptRetryLogState::default();

        assert!(state.should_warn());
        state.reset();

        assert!(state.should_warn());
    }
}
