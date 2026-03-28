use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, ProxyError, RouteDecisionError, TransportError};
use crate::proxy::http::ProxyHandle;
use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::runtime::tasks::keepalive::KeepaliveHandle;
use crate::target::TargetAddr;
use crate::transport::device::PacketDevice;
use crate::transport::{TransportStack, VpnStream, VpnUdpSocket};
use crate::{RouteProtocol, domain::route_match};

mod runtime;

use runtime::SessionRuntime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePlan {
    VpnResolved(SocketAddr),
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
        self.domain_rules
            .iter()
            .any(|(domain, rule)| route_match::domain_rule_matches(host, port, protocol, domain, rule))
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
    client_ip: Ipv4Addr,
    resources: ResourceSet,
    local_route_overrides: LocalRouteOverrides,
    allow_all_routes: bool,
    resolver: SessionResolver,
    transport: TransportStack,
    legacy_data_plane: Option<LegacyDataPlaneConfig>,
    runtime: Arc<SessionRuntime>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct LegacyDataPlaneConfig {
    server_addr: SocketAddr,
    token: crate::protocol::DerivedToken,
    legacy_cipher_hint: Option<String>,
}

impl EasyConnectSession {
    pub fn new(
        client_ip: Ipv4Addr,
        resources: ResourceSet,
        resolver: SessionResolver,
        transport: TransportStack,
    ) -> Self {
        Self {
            client_ip,
            resources,
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            resolver,
            transport,
            legacy_data_plane: None,
            runtime: Arc::new(SessionRuntime::default()),
        }
    }

    pub(crate) fn with_legacy_data_plane(
        mut self,
        server_addr: SocketAddr,
        token: crate::protocol::DerivedToken,
        legacy_cipher_hint: Option<String>,
    ) -> Self {
        self.legacy_data_plane = Some(LegacyDataPlaneConfig {
            server_addr,
            token,
            legacy_cipher_hint,
        });
        self
    }

    pub(crate) fn with_runtime_resources(
        mut self,
        legacy_tunnel: Option<smelly_tls::TunnelConnection>,
        keepalive: Option<KeepaliveHandle>,
    ) -> Self {
        self.runtime = Arc::new(SessionRuntime::new(legacy_tunnel, keepalive));
        self
    }

    pub fn client_ip(&self) -> Ipv4Addr {
        self.client_ip
    }

    pub fn resources(&self) -> &ResourceSet {
        &self.resources
    }

    pub fn local_route_overrides(&self) -> &LocalRouteOverrides {
        &self.local_route_overrides
    }

    pub fn with_local_route_overrides(mut self, overrides: LocalRouteOverrides) -> Self {
        self.local_route_overrides = overrides;
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
            !self.resources.matches_ip(IpAddr::V4(ip), port, RouteProtocol::Tcp)
                && !self
                    .local_route_overrides
                    .matches_ip(IpAddr::V4(ip), port, RouteProtocol::Tcp)
        } else {
            !self.resources.matches_domain(host, port, RouteProtocol::Tcp)
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
        let transport = self.transport.clone();
        let resolver = self.resolver.clone();
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
        resolve_keepalive_target(&self.resolver, &target)
            .await
            .map_err(Error::Resolve)
    }

    pub async fn icmp_ping_ip(&self, target: Ipv4Addr) -> Result<(), Error> {
        self.transport
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
        let route = self.plan_tcp_connect(target).await?;
        match route {
            RoutePlan::VpnResolved(addr) => self
                .transport
                .connect(addr)
                .await
                .map_err(|err| Error::Transport(TransportError::from_io(err))),
        }
    }

    pub async fn bind_udp(&self) -> Result<SessionUdpSocket, Error> {
        let socket = self
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
        let transport = self.transport.clone();
        let resolver = self.resolver.clone();
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
        let cfg = self.legacy_data_plane.as_ref().ok_or_else(|| {
            Error::Transport(TransportError::ConnectFailed(
                "legacy data plane unavailable".to_string(),
            ))
        })?;
        crate::auth::control::spawn_legacy_packet_device(
            cfg.server_addr,
            &cfg.token,
            self.client_ip,
            cfg.legacy_cipher_hint.as_deref(),
        )
        .await
    }

    pub async fn plan_tcp_connect<T>(&self, target: T) -> Result<RoutePlan, Error>
    where
        T: Into<TargetAddr>,
    {
        let addr = self.plan_socket_addr(target, RouteProtocol::Tcp).await?;
        Ok(RoutePlan::VpnResolved(addr))
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

        if !self.allow_all_routes
            && !self.resources.matches_domain(&host, port, protocol)
            && !self.local_route_overrides.matches_domain(&host, port, protocol)
        {
            return Err(Error::RouteDecision(RouteDecisionError::TargetNotAllowed));
        }

        let ip = self
            .resolver
            .resolve_for_vpn(&host)
            .await
            .map_err(Error::Resolve)?;

        Ok(SocketAddr::new(ip, port))
    }

    fn plan_ip(&self, ip: Ipv4Addr, port: u16, protocol: RouteProtocol) -> Result<SocketAddr, Error> {
        if !self.allow_all_routes
            && !self.resources.matches_ip(IpAddr::V4(ip), port, protocol)
            && !self.local_route_overrides.matches_ip(IpAddr::V4(ip), port, protocol)
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
