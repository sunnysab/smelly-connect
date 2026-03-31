use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tracing::{info, warn};

use crate::domain::route_policy::RoutePolicy;
use crate::error::{Error, ProxyError, RouteDecisionError, TransportError};
use crate::proxy::http::ProxyHandle;
use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::runtime::tasks::keepalive::KeepaliveHandle;
use crate::target::TargetAddr;
use crate::transport::device::PacketDevice;
use crate::transport::{TransportStack, VpnStream, VpnUdpSocket};
use crate::{RouteProtocol, domain::route_match};

mod inner;
mod runtime;

use inner::{LegacyDataPlaneConfig, SessionInner};
use runtime::SessionRuntime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePlan {
    VpnResolved(SocketAddr),
    Direct(SocketAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcmpKeepAliveTarget {
    Ip(Ipv4Addr),
    Host(String),
}

#[derive(Clone)]
pub struct SessionUdpSocket {
    session: EasyConnectSession,
    socket: VpnUdpSocket,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalRouteOverrides {
    domain_rules: HashMap<String, DomainRule>,
    ip_rules: Vec<IpRule>,
}

impl LocalRouteOverrides {
    pub fn new(domain_rules: HashMap<String, DomainRule>, ip_rules: Vec<IpRule>) -> Self {
        let domain_rules = domain_rules
            .into_iter()
            .map(|(domain, rule)| (normalize_override_domain(&domain), rule))
            .collect();
        Self {
            domain_rules,
            ip_rules,
        }
    }

    pub fn domain_rules(&self) -> &HashMap<String, DomainRule> {
        &self.domain_rules
    }

    pub fn ip_rules(&self) -> &[IpRule] {
        &self.ip_rules
    }

    fn matches_domain(&self, host: &str, port: u16, protocol: RouteProtocol) -> bool {
        self.domain_rules.iter().any(|(domain, rule)| {
            route_match::domain_rule_matches(host, port, protocol, domain, rule)
        })
    }

    fn matches_ip(&self, ip: IpAddr, port: u16, protocol: RouteProtocol) -> bool {
        self.ip_rules
            .iter()
            .any(|rule| route_match::ip_rule_matches(ip, port, protocol, rule))
    }
}

fn normalize_override_domain(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("*.") {
        format!(".{rest}")
    } else {
        trimmed.to_string()
    }
}

#[derive(Clone)]
pub struct EasyConnectSession {
    inner: Arc<SessionInner>,
    local_route_overrides: LocalRouteOverrides,
    route_policy: RoutePolicy,
    allow_all_routes: bool,
}

impl EasyConnectSession {
    pub fn new(
        client_ip: Ipv4Addr,
        resources: ResourceSet,
        resolver: SessionResolver,
        transport: TransportStack,
    ) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                client_ip,
                resources,
                resolver,
                transport,
                legacy_data_plane: None,
                runtime: Arc::new(SessionRuntime::default()),
            }),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
        }
    }

    pub(crate) fn with_legacy_data_plane(
        mut self,
        server_addr: SocketAddr,
        token: crate::protocol::DerivedToken,
        legacy_cipher_hint: Option<String>,
    ) -> Self {
        Arc::make_mut(&mut self.inner).legacy_data_plane = Some(LegacyDataPlaneConfig {
            server_addr,
            token,
            legacy_cipher_hint,
            #[cfg(any(test, debug_assertions))]
            transport_rebuilder: None,
        });
        self
    }

    #[cfg(any(test, debug_assertions))]
    pub fn with_transport_rebuild_for_test<F>(mut self, rebuilder: F) -> Self
    where
        F: Fn() -> Result<TransportStack, Error> + Send + Sync + 'static,
    {
        let server_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 443));
        let token = crate::protocol::DerivedToken([0_u8; 48]);
        Arc::make_mut(&mut self.inner).legacy_data_plane = Some(LegacyDataPlaneConfig {
            server_addr,
            token,
            legacy_cipher_hint: None,
            transport_rebuilder: Some(Arc::new(rebuilder)),
        });
        self
    }

    pub(crate) fn with_runtime_resources(
        mut self,
        legacy_tunnel: Option<smelly_tls::TunnelConnection>,
        keepalive: Option<KeepaliveHandle>,
    ) -> Self {
        Arc::make_mut(&mut self.inner).runtime =
            Arc::new(SessionRuntime::new(legacy_tunnel, keepalive));
        self
    }

    pub fn client_ip(&self) -> Ipv4Addr {
        self.inner.client_ip
    }

    pub fn resources(&self) -> &ResourceSet {
        &self.inner.resources
    }

    pub fn local_route_overrides(&self) -> &LocalRouteOverrides {
        &self.local_route_overrides
    }

    pub fn with_local_route_overrides(mut self, overrides: LocalRouteOverrides) -> Self {
        self.local_route_overrides = overrides;
        self
    }

    pub fn with_route_policy(mut self, route_policy: RoutePolicy) -> Self {
        self.route_policy = route_policy;
        self
    }

    pub fn with_allow_all_routes(mut self, allow_all_routes: bool) -> Self {
        self.allow_all_routes = allow_all_routes;
        self
    }

    pub fn is_allow_all_bypass_target<T>(&self, target: T) -> bool
    where
        T: Into<TargetAddr>,
    {
        if !self.allow_all_routes {
            return false;
        }

        let target = target.into();
        let host = target.host();
        let port = target.port();
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            !self
                .inner
                .resources
                .matches_ip(IpAddr::V4(ip), port, RouteProtocol::Tcp)
                && !self
                    .local_route_overrides
                    .matches_ip(IpAddr::V4(ip), port, RouteProtocol::Tcp)
        } else {
            !self
                .inner
                .resources
                .matches_domain(host, port, RouteProtocol::Tcp)
                && !self
                    .local_route_overrides
                    .matches_domain(host, port, RouteProtocol::Tcp)
        }
    }

    pub fn spawn_icmp_keepalive_task(
        &self,
        target: IcmpKeepAliveTarget,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let transport = self.inner.transport.clone();
        let resolver = self.inner.resolver.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(ip) = resolve_keepalive_target(&resolver, &target).await {
                    let _ = transport.icmp_ping(ip).await;
                }
                tokio::time::sleep(interval).await;
            }
        })
    }

    pub async fn resolve_icmp_target(
        &self,
        target: IcmpKeepAliveTarget,
    ) -> Result<Ipv4Addr, Error> {
        resolve_keepalive_target(&self.inner.resolver, &target)
            .await
            .map_err(Error::Resolve)
    }

    pub async fn icmp_ping_ip(&self, target: Ipv4Addr) -> Result<(), Error> {
        self.inner
            .transport
            .icmp_ping(target)
            .await
            .map_err(|err| Error::Transport(TransportError::from_io(err)))
    }

    pub async fn icmp_ping(&self, target: IcmpKeepAliveTarget) -> Result<(), Error> {
        let ip = self.resolve_icmp_target(target).await?;
        self.icmp_ping_ip(ip).await
    }

    pub async fn connect_tcp<T>(&self, target: T) -> Result<VpnStream, Error>
    where
        T: Into<TargetAddr>,
    {
        let target = target.into();
        let host = target.host().to_string();
        let port = target.port();
        let started = std::time::Instant::now();
        let route = match self.plan_tcp_connect((host.as_str(), port)).await {
            Ok(route) => route,
            Err(err) => {
                warn!(
                    target_host = %host,
                    target_port = port,
                    elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    error_kind = session_plan_error_kind(&err),
                    error = ?err,
                    "session tcp connect planning failed"
                );
                return Err(err);
            }
        };
        match route {
            RoutePlan::VpnResolved(addr) => {
                info!(
                    route_kind = "vpn",
                    target_host = %host,
                    target_port = port,
                    resolved_addr = %addr,
                    plan_elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    "session tcp connect planned"
                );
                let _connect_permit = self.inner.runtime.acquire_connect_permit().await;
                let transport_started = std::time::Instant::now();
                match self.inner.transport.connect(addr).await {
                    Ok(stream) => {
                        info!(
                            route_kind = "vpn",
                            target_host = %host,
                            target_port = port,
                            resolved_addr = %addr,
                            connect_elapsed_ms = transport_started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            total_elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            "session tcp connect established"
                        );
                        Ok(stream)
                    }
                    Err(err) => {
                        let mapped = Error::Transport(TransportError::from_io(err));
                        warn!(
                            route_kind = "vpn",
                            target_host = %host,
                            target_port = port,
                            resolved_addr = %addr,
                            connect_elapsed_ms = transport_started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            total_elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            error_kind = session_transport_error_kind(&mapped),
                            error = ?mapped,
                            "session tcp connect failed"
                        );
                        Err(mapped)
                    }
                }
            }
            RoutePlan::Direct(addr) => {
                info!(
                    route_kind = "direct",
                    target_host = %host,
                    target_port = port,
                    resolved_addr = %addr,
                    plan_elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    "session tcp connect planned"
                );
                let transport_started = std::time::Instant::now();
                match TcpStream::connect(addr).await {
                    Ok(stream) => {
                        info!(
                            route_kind = "direct",
                            target_host = %host,
                            target_port = port,
                            resolved_addr = %addr,
                            connect_elapsed_ms = transport_started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            total_elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            "session tcp connect established"
                        );
                        Ok(VpnStream::new(stream))
                    }
                    Err(err) => {
                        let mapped = Error::Transport(TransportError::from_io(err));
                        warn!(
                            route_kind = "direct",
                            target_host = %host,
                            target_port = port,
                            resolved_addr = %addr,
                            connect_elapsed_ms = transport_started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            total_elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                            error_kind = session_transport_error_kind(&mapped),
                            error = ?mapped,
                            "session tcp connect failed"
                        );
                        Err(mapped)
                    }
                }
            }
        }
    }

    pub async fn bind_udp(&self) -> Result<SessionUdpSocket, Error> {
        let socket = self
            .inner
            .transport
            .bind_udp()
            .await
            .map_err(|err| Error::Transport(TransportError::from_io(err)))?;
        Ok(SessionUdpSocket {
            session: self.clone(),
            socket,
        })
    }

    pub async fn start_http_proxy(&self, bind: SocketAddr) -> Result<ProxyHandle, Error> {
        crate::integration::http_proxy::start_http_proxy(self.clone(), bind)
            .await
            .map_err(|err| Error::Proxy(ProxyError::BindFailed(err.to_string())))
    }

    pub async fn rebuild_transport(&self) -> Result<Self, Error> {
        let cfg = self.inner.legacy_data_plane.as_ref().ok_or_else(|| {
            Error::Transport(TransportError::ConnectFailed(
                "legacy data plane unavailable".to_string(),
            ))
        })?;
        #[cfg(any(test, debug_assertions))]
        let (client_ip, transport, request_ip_tunnel) = if let Some(rebuilder) = &cfg.transport_rebuilder {
            (self.inner.client_ip, rebuilder()?, None)
        } else {
            let (client_ip, request_ip_tunnel) =
                crate::auth::control::request_ip_via_tunnel_with_conn(
                    cfg.server_addr,
                    &cfg.token,
                    cfg.legacy_cipher_hint.as_deref(),
                )
                .await?;
            let device = crate::auth::control::spawn_legacy_packet_device(
                cfg.server_addr,
                &cfg.token,
                client_ip,
                cfg.legacy_cipher_hint.as_deref(),
            )
            .await?;
            let transport = crate::transport::netstack::build_transport_from_packet_device(
                device,
                client_ip,
            )
            .map_err(|err| Error::Transport(TransportError::from_io(err)))?;
            (client_ip, transport, Some(request_ip_tunnel))
        };
        #[cfg(not(any(test, debug_assertions)))]
        let (client_ip, transport, request_ip_tunnel) = {
            let (client_ip, request_ip_tunnel) =
                crate::auth::control::request_ip_via_tunnel_with_conn(
                    cfg.server_addr,
                    &cfg.token,
                    cfg.legacy_cipher_hint.as_deref(),
                )
                .await?;
            let device = crate::auth::control::spawn_legacy_packet_device(
                cfg.server_addr,
                &cfg.token,
                client_ip,
                cfg.legacy_cipher_hint.as_deref(),
            )
            .await?;
            let transport =
                crate::transport::netstack::build_transport_from_packet_device(device, client_ip)
                    .map_err(|err| Error::Transport(TransportError::from_io(err)))?;
            (client_ip, transport, Some(request_ip_tunnel))
        };

        Ok(EasyConnectSession::new(
            client_ip,
            self.inner.resources.clone(),
            self.inner.resolver.clone(),
            transport,
        )
        .with_local_route_overrides(self.local_route_overrides.clone())
        .with_route_policy(self.route_policy)
        .with_allow_all_routes(self.allow_all_routes)
        .with_legacy_data_plane(
            cfg.server_addr,
            cfg.token.clone(),
            cfg.legacy_cipher_hint.clone(),
        )
        .with_runtime_resources(request_ip_tunnel, None))
    }

    pub async fn rebuild_transport_from_existing_lease(self) -> Result<Self, Error> {
        let cfg = self.inner.legacy_data_plane.as_ref().ok_or_else(|| {
            Error::Transport(TransportError::ConnectFailed(
                "legacy data plane unavailable".to_string(),
            ))
        })?;
        let server_addr = cfg.server_addr;
        let token = cfg.token.clone();
        let legacy_cipher_hint = cfg.legacy_cipher_hint.clone();
        #[cfg(any(test, debug_assertions))]
        let transport_rebuilder = cfg.transport_rebuilder.clone();
        let client_ip = self.inner.client_ip;
        let resources = self.inner.resources.clone();
        let resolver = self.inner.resolver.clone();
        let local_route_overrides = self.local_route_overrides.clone();
        let route_policy = self.route_policy;
        let allow_all_routes = self.allow_all_routes;
        let request_ip_tunnel = self.inner.runtime.take_legacy_tunnel();

        drop(self);
        tokio::time::sleep(Duration::from_millis(500)).await;

        #[cfg(any(test, debug_assertions))]
        let (transport, request_ip_tunnel) = if let Some(rebuilder) = transport_rebuilder {
            (rebuilder()?, None)
        } else if let Some(request_ip_tunnel) = request_ip_tunnel {
            let recv = crate::auth::control::open_recv_tunnel(
                server_addr,
                &token,
                client_ip,
                legacy_cipher_hint.as_deref(),
            )
            .await?;
            let send = crate::auth::control::open_send_tunnel(
                server_addr,
                &token,
                client_ip,
                legacy_cipher_hint.as_deref(),
            )
            .await?;
            let device = crate::auth::control::packet_device_from_tunnels(recv, send)?;
            let transport =
                crate::transport::netstack::build_transport_from_packet_device(device, client_ip)
                    .map_err(|err| Error::Transport(TransportError::from_io(err)))?;
            (transport, Some(request_ip_tunnel))
        } else {
            let rebuilt = EasyConnectSession::new(
                client_ip,
                resources.clone(),
                resolver.clone(),
                crate::transport::TransportStack::new(|_| async {
                    Err(std::io::Error::other("placeholder transport"))
                }),
            )
            .with_local_route_overrides(local_route_overrides.clone())
            .with_route_policy(route_policy)
            .with_allow_all_routes(allow_all_routes)
            .with_legacy_data_plane(server_addr, token.clone(), legacy_cipher_hint.clone())
            .rebuild_transport()
            .await?;
            return Ok(rebuilt);
        };
        #[cfg(not(any(test, debug_assertions)))]
        let (transport, request_ip_tunnel) = if let Some(request_ip_tunnel) = request_ip_tunnel {
            let recv = crate::auth::control::open_recv_tunnel(
                server_addr,
                &token,
                client_ip,
                legacy_cipher_hint.as_deref(),
            )
            .await?;
            let send = crate::auth::control::open_send_tunnel(
                server_addr,
                &token,
                client_ip,
                legacy_cipher_hint.as_deref(),
            )
            .await?;
            let device = crate::auth::control::packet_device_from_tunnels(recv, send)?;
            let transport =
                crate::transport::netstack::build_transport_from_packet_device(device, client_ip)
                    .map_err(|err| Error::Transport(TransportError::from_io(err)))?;
            (transport, Some(request_ip_tunnel))
        } else {
            let rebuilt = EasyConnectSession::new(
                client_ip,
                resources.clone(),
                resolver.clone(),
                crate::transport::TransportStack::new(|_| async {
                    Err(std::io::Error::other("placeholder transport"))
                }),
            )
            .with_local_route_overrides(local_route_overrides.clone())
            .with_route_policy(route_policy)
            .with_allow_all_routes(allow_all_routes)
            .with_legacy_data_plane(server_addr, token.clone(), legacy_cipher_hint.clone())
            .rebuild_transport()
            .await?;
            return Ok(rebuilt);
        };

        Ok(EasyConnectSession::new(
            client_ip,
            resources,
            resolver,
            transport,
        )
        .with_local_route_overrides(local_route_overrides)
        .with_route_policy(route_policy)
        .with_allow_all_routes(allow_all_routes)
        .with_legacy_data_plane(server_addr, token, legacy_cipher_hint)
        .with_runtime_resources(request_ip_tunnel, None))
    }

    pub fn start_icmp_keepalive<T>(&self, target: T, interval: Duration) -> KeepaliveHandle
    where
        T: Into<IcmpKeepAliveTarget>,
    {
        self.start_icmp_keepalive_with_failure_handler(target, interval, || {})
    }

    pub fn start_icmp_keepalive_with_failure_handler<T, F>(
        &self,
        target: T,
        interval: Duration,
        on_failure: F,
    ) -> KeepaliveHandle
    where
        T: Into<IcmpKeepAliveTarget>,
        F: Fn() + Send + Sync + 'static,
    {
        let target = target.into();
        let transport = self.inner.transport.clone();
        let resolver = self.inner.resolver.clone();
        let on_failure = Arc::new(on_failure);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = async {
                        let failed = match resolve_keepalive_target(&resolver, &target).await {
                            Ok(ip) => transport.icmp_ping(ip).await.is_err(),
                            Err(_) => true,
                        };
                        if failed {
                            on_failure();
                        }
                        tokio::time::sleep(interval).await;
                    } => {}
                }
            }
        });
        KeepaliveHandle {
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        }
    }

    pub async fn reqwest_client(&self) -> Result<reqwest::Client, Error> {
        crate::integration::reqwest::build_client(self).await
    }

    #[allow(dead_code)]
    pub(crate) async fn spawn_packet_device(&self) -> Result<PacketDevice, Error> {
        let cfg = self.inner.legacy_data_plane.as_ref().ok_or_else(|| {
            Error::Transport(TransportError::ConnectFailed(
                "legacy data plane unavailable".to_string(),
            ))
        })?;
        crate::auth::control::spawn_legacy_packet_device(
            cfg.server_addr,
            &cfg.token,
            self.inner.client_ip,
            cfg.legacy_cipher_hint.as_deref(),
        )
        .await
    }

    pub async fn plan_tcp_connect<T>(&self, target: T) -> Result<RoutePlan, Error>
    where
        T: Into<TargetAddr>,
    {
        let target = target.into();
        let host = target.host().to_string();
        let port = target.port();

        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return self.plan_tcp_ip(ip, port);
        }

        let ip = self
            .inner
            .resolver
            .resolve_for_vpn(&host)
            .await
            .map_err(Error::Resolve)?;
        self.plan_tcp_host(&host, ip, port)
    }

    fn plan_tcp_host(&self, host: &str, ip: IpAddr, port: u16) -> Result<RoutePlan, Error> {
        if self.allow_all_routes
            || self.inner.resources.matches_domain(host, port, RouteProtocol::Tcp)
            || self
                .local_route_overrides
                .matches_domain(host, port, RouteProtocol::Tcp)
            || self.inner.resources.matches_ip(ip, port, RouteProtocol::Tcp)
            || self
                .local_route_overrides
                .matches_ip(ip, port, RouteProtocol::Tcp)
        {
            Ok(RoutePlan::VpnResolved(SocketAddr::new(ip, port)))
        } else {
            self.plan_direct_or_block(SocketAddr::new(ip, port))
        }
    }

    fn plan_tcp_ip(&self, ip: Ipv4Addr, port: u16) -> Result<RoutePlan, Error> {
        let addr = SocketAddr::new(IpAddr::V4(ip), port);
        if self.allow_all_routes
            || self
                .inner
                .resources
                .matches_ip(IpAddr::V4(ip), port, RouteProtocol::Tcp)
            || self
                .local_route_overrides
                .matches_ip(IpAddr::V4(ip), port, RouteProtocol::Tcp)
        {
            Ok(RoutePlan::VpnResolved(addr))
        } else {
            self.plan_direct_or_block(addr)
        }
    }

    fn plan_direct_or_block(&self, addr: SocketAddr) -> Result<RoutePlan, Error> {
        match self.route_policy {
            RoutePolicy::DirectNonResourceTargets => Ok(RoutePlan::Direct(addr)),
            RoutePolicy::RejectNonResourceTargets => {
                Err(Error::RouteDecision(RouteDecisionError::TargetNotAllowed))
            }
        }
    }

    async fn plan_socket_addr<T>(
        &self,
        target: T,
        protocol: RouteProtocol,
    ) -> Result<SocketAddr, Error>
    where
        T: Into<TargetAddr>,
    {
        let target = target.into();
        let host = target.host().to_string();
        let port = target.port();

        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return self.plan_ip(ip, port, protocol);
        }

        let ip = self
            .inner
            .resolver
            .resolve_for_vpn(&host)
            .await
            .map_err(Error::Resolve)?;

        if !self.allow_all_routes
            && !self.inner.resources.matches_domain(&host, port, protocol)
            && !self
                .local_route_overrides
                .matches_domain(&host, port, protocol)
            && !self.inner.resources.matches_ip(ip, port, protocol)
            && !self
                .local_route_overrides
                .matches_ip(ip, port, protocol)
        {
            return Err(Error::RouteDecision(RouteDecisionError::TargetNotAllowed));
        }

        Ok(SocketAddr::new(ip, port))
    }

    fn plan_ip(
        &self,
        ip: Ipv4Addr,
        port: u16,
        protocol: RouteProtocol,
    ) -> Result<SocketAddr, Error> {
        if !self.allow_all_routes
            && !self
                .inner
                .resources
                .matches_ip(IpAddr::V4(ip), port, protocol)
            && !self
                .local_route_overrides
                .matches_ip(IpAddr::V4(ip), port, protocol)
        {
            return Err(Error::RouteDecision(RouteDecisionError::TargetNotAllowed));
        }

        Ok(SocketAddr::new(IpAddr::V4(ip), port))
    }

    pub fn failing_transport(message: &'static str) -> TransportStack {
        TransportStack::new(move |_| async move { Err(io::Error::other(message)) })
    }
}

impl SessionUdpSocket {
    pub async fn send_to<T>(&self, data: &[u8], target: T) -> Result<usize, Error>
    where
        T: Into<TargetAddr>,
    {
        let addr = self
            .session
            .plan_socket_addr(target, RouteProtocol::Udp)
            .await?;
        self.socket
            .send_to(data, addr)
            .await
            .map_err(|err| Error::Transport(TransportError::from_io(err)))
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), Error> {
        self.socket
            .recv_from(buf)
            .await
            .map_err(|err| Error::Transport(TransportError::from_io(err)))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Error> {
        self.socket
            .local_addr()
            .map_err(|err| Error::Transport(TransportError::from_io(err)))
    }
}

fn session_plan_error_kind(err: &Error) -> &'static str {
    match err {
        Error::RouteDecision(RouteDecisionError::TargetNotAllowed) => "route_rejected",
        Error::Resolve(_) => "resolve_failed",
        Error::Transport(TransportError::ConnectTimedOut) => "transport_connect_timeout",
        Error::Transport(TransportError::ConnectFailed(_)) => "transport_connect_failed",
        Error::Transport(TransportError::ConnectionClosed) => "transport_connection_closed",
        _ => "plan_failed",
    }
}

fn session_transport_error_kind(err: &Error) -> &'static str {
    match err {
        Error::Transport(TransportError::ConnectTimedOut) => "transport_connect_timeout",
        Error::Transport(TransportError::ConnectFailed(_)) => "transport_connect_failed",
        Error::Transport(TransportError::ConnectionClosed) => "transport_connection_closed",
        _ => "transport_failed",
    }
}

async fn resolve_keepalive_target(
    resolver: &SessionResolver,
    target: &IcmpKeepAliveTarget,
) -> Result<Ipv4Addr, crate::error::ResolveError> {
    match target {
        IcmpKeepAliveTarget::Ip(ip) => Ok(*ip),
        IcmpKeepAliveTarget::Host(host) => match resolver.resolve_for_vpn(host).await? {
            IpAddr::V4(ip) => Ok(ip),
            IpAddr::V6(_) => Err(crate::error::ResolveError::NoRecordFound),
        },
    }
}

impl From<Ipv4Addr> for IcmpKeepAliveTarget {
    fn from(value: Ipv4Addr) -> Self {
        Self::Ip(value)
    }
}

impl From<String> for IcmpKeepAliveTarget {
    fn from(value: String) -> Self {
        match value.parse() {
            Ok(ip) => Self::Ip(ip),
            Err(_) => Self::Host(value),
        }
    }
}

impl From<&str> for IcmpKeepAliveTarget {
    fn from(value: &str) -> Self {
        value.to_string().into()
    }
}
