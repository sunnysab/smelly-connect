use std::collections::HashMap;
use std::net::IpAddr;

use crate::domain::route_match::{domain_rule_matches, ip_rule_matches};
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
    pub fn matches_domain(&self, host: &str, port: u16, protocol: RouteProtocol) -> bool {
        self.domain_rules
            .iter()
            .any(|(domain, rule)| domain_rule_matches(host, port, protocol, domain, rule))
    }

    pub fn matches_ip(&self, ip: IpAddr, port: u16, protocol: RouteProtocol) -> bool {
        self.ip_rules
            .iter()
            .any(|rule| ip_rule_matches(ip, port, protocol, rule))
    }
}
