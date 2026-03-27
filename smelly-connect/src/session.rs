use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::duplex;

use crate::config::EasyConnectConfig;
use crate::error::{Error, ProxyError, RouteDecisionError, TransportError};
use crate::proxy::http::ProxyHandle;
use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::runtime::tasks::keepalive::KeepaliveHandle;
use crate::target::TargetAddr;
use crate::transport::device::PacketDevice;
use crate::transport::{TransportStack, VpnStream, VpnUdpSocket};

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

    fn matches_domain(&self, host: &str, port: u16) -> bool {
        self.domain_rules.iter().any(|(domain, rule)| {
            port >= rule.port_min
                && port <= rule.port_max
                && if domain.starts_with('.') {
                    host.ends_with(domain)
                } else {
                    host == domain || host.ends_with(&format!(".{domain}"))
                }
        })
    }

    fn matches_ip(&self, ip: IpAddr, port: u16) -> bool {
        self.ip_rules.iter().any(|rule| {
            port >= rule.port_min
                && port <= rule.port_max
                && match (rule.ip_min, rule.ip_max, ip) {
                    (IpAddr::V4(min), IpAddr::V4(max), IpAddr::V4(current)) => {
                        current >= min && current <= max
                    }
                    (IpAddr::V6(min), IpAddr::V6(max), IpAddr::V6(current)) => {
                        current >= min && current <= max
                    }
                    _ => false,
                }
        })
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
            !self.resources.matches_ip(IpAddr::V4(ip), port)
                && !self.local_route_overrides.matches_ip(IpAddr::V4(ip), port)
        } else {
            !self.resources.matches_domain(host, port)
                && !self.local_route_overrides.matches_domain(host, port)
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
            .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))
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
                .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string()))),
        }
    }

    pub async fn bind_udp(&self) -> Result<SessionUdpSocket, Error> {
        let socket = self
            .transport
            .bind_udp()
            .await
            .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))?;
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
        let target = target.into();
        let transport = self.transport.clone();
        let resolver = self.resolver.clone();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = async {
                        if let Ok(ip) = resolve_keepalive_target(&resolver, &target).await {
                            let _ = transport.icmp_ping(ip).await;
                        }
                        tokio::time::sleep(interval).await;
                    } => {}
                }
            }
        });
        KeepaliveHandle {
            shutdown_tx: Some(shutdown_tx),
            task,
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
        let addr = self.plan_socket_addr(target).await?;
        Ok(RoutePlan::VpnResolved(addr))
    }

    async fn plan_socket_addr<T>(&self, target: T) -> Result<SocketAddr, Error>
    where
        T: Into<TargetAddr>,
    {
        let target = target.into();
        let host = target.host().to_string();
        let port = target.port();

        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return self.plan_ip(ip, port);
        }

        if !self.allow_all_routes
            && !self.resources.matches_domain(&host, port)
            && !self.local_route_overrides.matches_domain(&host, port)
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

    fn plan_ip(&self, ip: Ipv4Addr, port: u16) -> Result<SocketAddr, Error> {
        if !self.allow_all_routes
            && !self.resources.matches_ip(IpAddr::V4(ip), port)
            && !self.local_route_overrides.matches_ip(IpAddr::V4(ip), port)
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
        let addr = self.session.plan_socket_addr(target).await?;
        self.socket
            .send_to(data, addr)
            .await
            .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), Error> {
        self.socket
            .recv_from(buf)
            .await
            .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Error> {
        self.socket
            .local_addr()
            .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))
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

pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::UdpSocket;

    pub fn fake_session_without_match() -> EasyConnectSession {
        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
            ready_transport(),
        )
    }

    pub fn fake_session_without_match_with_transport(
        transport: TransportStack,
    ) -> EasyConnectSession {
        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
            transport,
        )
    }

    pub struct LoginHarness {
        host: Arc<str>,
        ip: Ipv4Addr,
    }

    impl LoginHarness {
        pub fn config(&self) -> EasyConnectConfig {
            let host = Arc::clone(&self.host);
            let ip = self.ip;
            let mut config = EasyConnectConfig::new("rvpn.example.com", "user", "pass");
            config.session_factory = Some(Arc::new(move || Ok(ready_session(host.as_ref(), ip))));
            config
        }

        pub async fn ready_session(&self) -> EasyConnectSession {
            ready_session(self.host.as_ref(), self.ip)
        }
    }

    pub fn login_harness() -> LoginHarness {
        LoginHarness {
            host: Arc::<str>::from("libdb.zju.edu.cn"),
            ip: Ipv4Addr::new(10, 0, 0, 8),
        }
    }

    #[allow(dead_code)]
    pub fn session_with_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
        ready_session(host, ip)
    }

    pub fn session_with_failing_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
        let mut resources = ResourceSet::default();
        resources.domain_rules.insert(
            host.to_string(),
            DomainRule {
                port_min: 1,
                port_max: 65535,
                protocol: "all".to_string(),
            },
        );
        resources.ip_rules.push(IpRule {
            ip_min: IpAddr::V4(ip),
            ip_max: IpAddr::V4(ip),
            port_min: 1,
            port_max: 65535,
            protocol: "all".to_string(),
        });
        resources
            .static_dns
            .insert(host.to_string(), IpAddr::V4(ip));

        let mut system = HashMap::new();
        system.insert(host.to_string(), IpAddr::V4(ip));

        EasyConnectSession::new(
            ip,
            resources,
            SessionResolver::new(HashMap::new(), None, system),
            EasyConnectSession::failing_transport("forced live connect failure"),
        )
    }

    pub fn session_with_slow_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
        let mut resources = ResourceSet::default();
        resources.domain_rules.insert(
            host.to_string(),
            DomainRule {
                port_min: 1,
                port_max: 65535,
                protocol: "all".to_string(),
            },
        );
        resources.ip_rules.push(IpRule {
            ip_min: IpAddr::V4(ip),
            ip_max: IpAddr::V4(ip),
            port_min: 1,
            port_max: 65535,
            protocol: "all".to_string(),
        });
        resources
            .static_dns
            .insert(host.to_string(), IpAddr::V4(ip));

        let mut system = HashMap::new();
        system.insert(host.to_string(), IpAddr::V4(ip));

        let transport = TransportStack::new(|_| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "forced slow connect",
            ))
        });

        EasyConnectSession::new(
            ip,
            resources,
            SessionResolver::new(HashMap::new(), None, system),
            transport,
        )
    }

    pub fn session_with_icmp_ping(counter: Arc<AtomicUsize>) -> EasyConnectSession {
        let transport = ready_transport().with_icmp_pinger(move |_| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
            transport,
        )
    }

    pub fn session_with_icmp_result(success: bool) -> EasyConnectSession {
        let transport = ready_transport().with_icmp_pinger(move |_| async move {
            if success {
                Ok(())
            } else {
                Err(io::Error::other("forced icmp failure"))
            }
        });

        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
            transport,
        )
    }

    pub fn session_with_delayed_icmp_result(
        success: bool,
        delay: Duration,
        counter: Arc<AtomicUsize>,
    ) -> EasyConnectSession {
        let transport = ready_transport().with_icmp_pinger(move |_| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                if success {
                    Ok(())
                } else {
                    Err(io::Error::other("forced delayed icmp failure"))
                }
            }
        });

        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
            transport,
        )
    }

    pub fn session_with_failing_domain_match_and_delayed_icmp(
        host: &str,
        ip: Ipv4Addr,
        delay: Duration,
        counter: Arc<AtomicUsize>,
    ) -> EasyConnectSession {
        let mut resources = ResourceSet::default();
        resources.domain_rules.insert(
            host.to_string(),
            DomainRule {
                port_min: 1,
                port_max: 65535,
                protocol: "all".to_string(),
            },
        );
        resources.ip_rules.push(IpRule {
            ip_min: IpAddr::V4(ip),
            ip_max: IpAddr::V4(ip),
            port_min: 1,
            port_max: 65535,
            protocol: "all".to_string(),
        });
        resources
            .static_dns
            .insert(host.to_string(), IpAddr::V4(ip));

        let mut system = HashMap::new();
        system.insert(host.to_string(), IpAddr::V4(ip));

        let transport = EasyConnectSession::failing_transport("forced live connect failure")
            .with_icmp_pinger(move |_| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    Err(io::Error::other("forced delayed icmp failure"))
                }
            });

        EasyConnectSession::new(
            ip,
            resources,
            SessionResolver::new(HashMap::new(), None, system),
            transport,
        )
    }

    fn ready_session(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
        let mut resources = ResourceSet::default();
        resources.domain_rules.insert(
            host.to_string(),
            DomainRule {
                port_min: 1,
                port_max: 65535,
                protocol: "all".to_string(),
            },
        );
        resources.ip_rules.push(IpRule {
            ip_min: IpAddr::V4(ip),
            ip_max: IpAddr::V4(ip),
            port_min: 1,
            port_max: 65535,
            protocol: "all".to_string(),
        });
        resources
            .static_dns
            .insert(host.to_string(), IpAddr::V4(ip));

        let mut system = HashMap::new();
        system.insert(host.to_string(), IpAddr::V4(ip));

        EasyConnectSession::new(
            ip,
            resources,
            SessionResolver::new(HashMap::new(), None, system),
            ready_transport(),
        )
    }

    fn ready_transport() -> TransportStack {
        TransportStack::new(|_| async {
            let (client, _server) = duplex(1024);
            Ok(VpnStream::new(client))
        })
        .with_udp_binder(|| async {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
            Ok(VpnUdpSocket::new(socket))
        })
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
