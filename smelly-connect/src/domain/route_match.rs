use std::net::IpAddr;

use crate::resource::{DomainRule, IpRule};
use crate::RouteProtocol;

pub fn domain_rule_matches(
    host: &str,
    port: u16,
    protocol: RouteProtocol,
    domain: &str,
    rule: &DomainRule,
) -> bool {
    route_protocol_matches(protocol, rule.protocol)
        && port >= rule.port_min
        && port <= rule.port_max
        && domain_matches(host, domain)
}

pub fn ip_rule_matches(
    ip: IpAddr,
    port: u16,
    protocol: RouteProtocol,
    rule: &IpRule,
) -> bool {
    route_protocol_matches(protocol, rule.protocol)
        && port >= rule.port_min
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
}

fn route_protocol_matches(request: RouteProtocol, rule: RouteProtocol) -> bool {
    matches!(rule, RouteProtocol::All) || request == rule
}

fn domain_matches(host: &str, domain: &str) -> bool {
    if domain.starts_with('.') {
        host.ends_with(domain)
    } else {
        host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }
}
