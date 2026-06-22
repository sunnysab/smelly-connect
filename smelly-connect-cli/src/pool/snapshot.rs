use serde::{Deserialize, Serialize};

use smelly_connect::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolHealthStatus {
    Healthy,
    Recovering,
    Down,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountNodeSnapshot {
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoolSummary {
    pub status: PoolHealthStatus,
    pub total_nodes: usize,
    pub selectable_nodes: usize,
    pub active_nodes: usize,
    pub connecting_nodes: usize,
    pub idle_nodes: usize,
    pub dead_nodes: usize,
    pub disabled_nodes: usize,
    pub total_reconnections: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoolSnapshot {
    #[serde(flatten)]
    pub summary: PoolSummary,
    pub nodes: Vec<AccountNodeSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRouteSnapshot {
    pub domain: String,
    pub port_min: u16,
    pub port_max: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRouteSnapshot {
    pub ip_min: String,
    pub ip_max: String,
    pub port_min: u16,
    pub port_max: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticDnsSnapshot {
    pub host: String,
    pub ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteSetSnapshot {
    pub domain_rules: Vec<DomainRouteSnapshot>,
    pub ip_rules: Vec<IpRouteSnapshot>,
    pub static_dns: Vec<StaticDnsSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRoutesSnapshot {
    pub name: String,
    pub state: String,
    pub routes: Option<RouteSetSnapshot>,
    pub local_routes: Option<RouteSetSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutesSnapshot {
    pub total_nodes: usize,
    pub nodes: Vec<AccountRoutesSnapshot>,
}

pub(super) fn build_route_set_snapshot(session: &Session) -> RouteSetSnapshot {
    let resources = session.resources();

    let mut domain_rules = resources
        .domain_rules
        .iter()
        .map(|(domain, rule)| DomainRouteSnapshot {
            domain: domain.clone(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.to_string(),
        })
        .collect::<Vec<_>>();
    domain_rules.sort_by(|a, b| a.domain.cmp(&b.domain));

    let mut ip_rules = resources
        .ip_rules
        .iter()
        .map(|rule| IpRouteSnapshot {
            ip_min: rule.ip_min.to_string(),
            ip_max: rule.ip_max.to_string(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.to_string(),
        })
        .collect::<Vec<_>>();
    ip_rules.sort_by(|a, b| {
        (&a.ip_min, &a.ip_max, a.port_min, a.port_max, &a.protocol).cmp(&(
            &b.ip_min,
            &b.ip_max,
            b.port_min,
            b.port_max,
            &b.protocol,
        ))
    });

    let mut static_dns = resources
        .static_dns
        .iter()
        .map(|(host, ip)| StaticDnsSnapshot {
            host: host.clone(),
            ip: ip.to_string(),
        })
        .collect::<Vec<_>>();
    static_dns.sort_by(|a, b| a.host.cmp(&b.host));

    RouteSetSnapshot {
        domain_rules,
        ip_rules,
        static_dns,
    }
}

pub(super) fn build_local_route_set_snapshot(session: &Session) -> RouteSetSnapshot {
    let local = session.local_route_overrides();

    let mut domain_rules = local
        .domain_rules()
        .iter()
        .map(|(domain, rule)| DomainRouteSnapshot {
            domain: domain.clone(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.to_string(),
        })
        .collect::<Vec<_>>();
    domain_rules.sort_by(|a, b| a.domain.cmp(&b.domain));

    let mut ip_rules = local
        .ip_rules()
        .iter()
        .map(|rule| IpRouteSnapshot {
            ip_min: rule.ip_min.to_string(),
            ip_max: rule.ip_max.to_string(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.to_string(),
        })
        .collect::<Vec<_>>();
    ip_rules.sort_by(|a, b| {
        (&a.ip_min, &a.ip_max, a.port_min, a.port_max, &a.protocol).cmp(&(
            &b.ip_min,
            &b.ip_max,
            b.port_min,
            b.port_max,
            &b.protocol,
        ))
    });

    RouteSetSnapshot {
        domain_rules,
        ip_rules,
        static_dns: vec![],
    }
}
