use std::net::{IpAddr, Ipv4Addr};

use roxmltree::Document;

use crate::resource::{DomainRule, IpRule, ResourceSet};

pub fn parse_resources(body: &str) -> Result<ResourceSet, roxmltree::Error> {
    let doc = Document::parse(body)?;
    let mut resources = ResourceSet::default();

    for rc in doc.descendants().filter(|n| n.has_tag_name("Rc")) {
        let protocol = match rc.attribute("proto").unwrap_or("-1") {
            "-1" => "all",
            "0" => "tcp",
            "1" => "udp",
            value => value,
        }
        .to_string();
        let hosts = rc.attribute("host").unwrap_or_default().split(';');
        let ports = rc.attribute("port").unwrap_or_default().split(';');

        for (host, port_range) in hosts.zip(ports) {
            let (port_min, port_max) = parse_port_range(port_range);
            if let Some((ip_min, ip_max)) = parse_ip_range(host) {
                resources.ip_rules.push(IpRule {
                    ip_min,
                    ip_max,
                    port_min,
                    port_max,
                    protocol: protocol.clone(),
                });
                continue;
            }

            let domain = normalize_domain(host);
            if !domain.is_empty() {
                resources.domain_rules.insert(
                    domain,
                    DomainRule {
                        port_min,
                        port_max,
                        protocol: protocol.clone(),
                    },
                );
            }
        }
    }

    if let Some(dns) = doc.descendants().find(|n| n.has_tag_name("Dns")) {
        if let Some(remote) = dns.attribute("dnsserver") {
            resources.remote_dns_server = Some(remote.to_string());
        }
        for entry in dns.attribute("data").unwrap_or_default().split(';') {
            let mut parts = entry.split(':');
            let _ = parts.next();
            let host = parts.next();
            let ip = parts.next();
            if let (Some(host), Some(ip)) = (host, ip)
                && let Ok(parsed) = ip.parse::<IpAddr>()
            {
                resources.static_dns.insert(host.to_string(), parsed);
            }
        }
    }

    Ok(resources)
}

fn parse_port_range(value: &str) -> (u16, u16) {
    let mut parts = value.split('~');
    let min = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1);
    let max = parts.next().and_then(|v| v.parse().ok()).unwrap_or(min);
    (min, max)
}

fn parse_ip_range(value: &str) -> Option<(IpAddr, IpAddr)> {
    let mut parts = value.split('~');
    let start = parts.next()?.parse::<Ipv4Addr>().ok()?;
    let end = parts.next()?.parse::<Ipv4Addr>().ok()?;
    Some((IpAddr::V4(start), IpAddr::V4(end)))
}

fn normalize_domain(value: &str) -> String {
    let trimmed = value
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    trimmed
        .split('/')
        .next()
        .unwrap_or_default()
        .trim_matches('*')
        .trim()
        .to_string()
}
