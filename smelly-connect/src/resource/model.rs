use std::collections::HashMap;
use std::net::IpAddr;

use crate::RouteProtocol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainRule {
    pub port_min: u16,
    pub port_max: u16,
    pub protocol: RouteProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpRule {
    pub ip_min: IpAddr,
    pub ip_max: IpAddr,
    pub port_min: u16,
    pub port_max: u16,
    pub protocol: RouteProtocol,
}

#[derive(Debug, Clone, Default)]
pub struct ResourceSet {
    pub domain_rules: HashMap<String, DomainRule>,
    pub ip_rules: Vec<IpRule>,
    pub static_dns: HashMap<String, IpAddr>,
    pub remote_dns_server: Option<String>,
}

impl ResourceSet {
    pub fn matches_domain(&self, host: &str, port: u16) -> bool {
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

    pub fn matches_ip(&self, ip: IpAddr, port: u16) -> bool {
        self.ip_rules.iter().any(|rule| {
            port >= rule.port_min
                && port <= rule.port_max
                && match (rule.ip_min, rule.ip_max, ip) {
                    (IpAddr::V4(min), IpAddr::V4(max), IpAddr::V4(current)) => {
                        current >= min && current <= max
                    }
                    _ => false,
                }
        })
    }
}
