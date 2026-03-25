use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::duplex;

use crate::config::EasyConnectConfig;
use crate::error::{Error, IntegrationError, ProxyError, RouteError, TransportError};
use crate::proxy::http::HttpProxyHandle;
use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::target::TargetAddr;
use crate::transport::device::PacketDevice;
use crate::transport::{TransportStack, VpnStream};

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
pub struct EasyConnectSession {
    client_ip: Ipv4Addr,
    resources: ResourceSet,
    resolver: SessionResolver,
    transport: TransportStack,
    legacy_data_plane: Option<LegacyDataPlaneConfig>,
}

#[derive(Clone)]
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
            resolver,
            transport,
            legacy_data_plane: None,
        }
    }

    pub fn with_legacy_data_plane(
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

    pub async fn icmp_ping(&self, target: IcmpKeepAliveTarget) -> Result<(), Error> {
        let ip = resolve_keepalive_target(&self.resolver, &target)
            .await
            .map_err(Error::Resolve)?;
        self.transport
            .icmp_ping(ip)
            .await
            .map_err(|err| Error::Transport(TransportError::ConnectFailed(err.to_string())))
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

    pub async fn start_http_proxy(&self, bind: SocketAddr) -> Result<HttpProxyHandle, Error> {
        crate::proxy::http::start_http_proxy(self.clone(), bind)
            .await
            .map_err(|err| Error::Proxy(ProxyError::BindFailed(err.to_string())))
    }

    pub async fn reqwest_client(&self) -> Result<reqwest::Client, Error> {
        let handle = self
            .start_http_proxy("127.0.0.1:0".parse().unwrap())
            .await?;
        let client = reqwest::Client::builder()
            .proxy(
                reqwest::Proxy::all(format!("http://{}", handle.local_addr())).map_err(|err| {
                    Error::Integration(IntegrationError::ClientBuildFailed(err.to_string()))
                })?,
            )
            .build()
            .map_err(|err| {
                Error::Integration(IntegrationError::ClientBuildFailed(err.to_string()))
            })?;
        std::mem::forget(handle);
        Ok(client)
    }

    pub async fn spawn_packet_device(&self) -> Result<PacketDevice, Error> {
        let cfg = self
            .legacy_data_plane
            .as_ref()
            .ok_or_else(|| Error::Transport(TransportError::ConnectFailed("legacy data plane unavailable".to_string())))?;
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
        let target = target.into();
        let host = target.host().to_string();
        let port = target.port();

        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return self.plan_ip(ip, port);
        }

        if !self.resources.matches_domain(&host, port) {
            return Err(Error::Route(RouteError::TargetNotAllowed));
        }

        let ip = self
            .resolver
            .resolve_for_vpn(&host)
            .await
            .map_err(Error::Resolve)?;

        Ok(RoutePlan::VpnResolved(SocketAddr::new(ip, port)))
    }

    fn plan_ip(&self, ip: Ipv4Addr, port: u16) -> Result<RoutePlan, Error> {
        if !self.resources.matches_ip(IpAddr::V4(ip), port) {
            return Err(Error::Route(RouteError::TargetNotAllowed));
        }

        Ok(RoutePlan::VpnResolved(SocketAddr::new(
            IpAddr::V4(ip),
            port,
        )))
    }

    pub fn failing_transport(message: &'static str) -> TransportStack {
        TransportStack::new(move |_| async move { Err(io::Error::other(message)) })
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

    pub fn fake_session_without_match() -> EasyConnectSession {
        EasyConnectSession::new(
            Ipv4Addr::new(10, 0, 0, 8),
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
            ready_transport(),
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
