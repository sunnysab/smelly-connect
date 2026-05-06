use std::time::Duration;

use tokio::net::TcpStream;

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

pub async fn connect_live_upstream_with_timeout(
    timeout: Duration,
    session: &smelly_connect::Session,
    host: &str,
    port: u16,
) -> Result<
    (smelly_connect::transport::VpnStream, LiveRouteBackend),
    (UpstreamConnectError, LiveRouteBackend),
> {
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
        smelly_connect::session::RoutePlan::VpnResolved(_) => {
            connect_session_with_timeout(timeout, session.connect_tcp((host, port)))
                .await
                .map(|upstream| (upstream, LiveRouteBackend::Vpn))
                .map_err(|err| (err, LiveRouteBackend::Vpn))
        }
        smelly_connect::session::RoutePlan::Direct(addr) => {
            connect_with_timeout(timeout, TcpStream::connect(addr))
                .await
                .map(|stream| {
                    (
                        smelly_connect::transport::VpnStream::new(stream),
                        LiveRouteBackend::Direct,
                    )
                })
                .map_err(|err| (err, LiveRouteBackend::Direct))
        }
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
