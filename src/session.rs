use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::error::{Error, RouteError};
use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::target::TargetAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePlan {
    VpnResolved(SocketAddr),
}

pub struct EasyConnectSession {
    resources: ResourceSet,
    resolver: SessionResolver,
}

impl EasyConnectSession {
    pub fn new(resources: ResourceSet, resolver: SessionResolver) -> Self {
        Self { resources, resolver }
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

        if !self.resources.matches_ip(ip, port) {
            return Err(Error::Route(RouteError::TargetNotAllowed));
        }

        Ok(RoutePlan::VpnResolved(SocketAddr::new(ip, port)))
    }

    fn plan_ip(&self, ip: Ipv4Addr, port: u16) -> Result<RoutePlan, Error> {
        if !self.resources.matches_ip(IpAddr::V4(ip), port) {
            return Err(Error::Route(RouteError::TargetNotAllowed));
        }

        Ok(RoutePlan::VpnResolved(SocketAddr::new(IpAddr::V4(ip), port)))
    }
}

pub mod tests {
    use super::*;

    pub fn fake_session_without_match() -> EasyConnectSession {
        EasyConnectSession::new(
            ResourceSet::default(),
            SessionResolver::new(HashMap::new(), None, HashMap::new()),
        )
    }

    #[allow(dead_code)]
    pub fn session_with_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
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

        EasyConnectSession::new(resources, SessionResolver::new(HashMap::new(), None, system))
    }
}
