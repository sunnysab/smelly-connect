use std::net::{IpAddr, Ipv4Addr};

use roxmltree::Document;

use super::messages::{LoginAuthChallenge, ResourceDocument};
use crate::resource::{DomainRule, IpRule, ResourceSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlParseError {
    MissingTag(&'static str),
    InvalidRsaExponent,
    MissingSuccessMarker,
    MissingTwfId,
}

pub fn parse_login_auth_challenge(body: &str) -> Result<LoginAuthChallenge, ControlParseError> {
    let twfid = extract_tag(body, "TwfID").ok_or(ControlParseError::MissingTag("TwfID"))?;
    let rsa_key_hex = extract_tag(body, "RSA_ENCRYPT_KEY")
        .ok_or(ControlParseError::MissingTag("RSA_ENCRYPT_KEY"))?;
    let rsa_exp = extract_tag(body, "RSA_ENCRYPT_EXP")
        .unwrap_or("65537")
        .parse()
        .map_err(|_| ControlParseError::InvalidRsaExponent)?;
    let csrf_rand_code = extract_tag(body, "CSRF_RAND_CODE").map(ToOwned::to_owned);
    let legacy_cipher_hint = extract_nested_tag(body, "SSLCipherSuite", "EC").map(ToOwned::to_owned);
    let requires_captcha = extract_tag(body, "RndImg") == Some("1");

    Ok(LoginAuthChallenge {
        twfid: twfid.to_owned(),
        rsa_key_hex: rsa_key_hex.to_owned(),
        rsa_exp,
        csrf_rand_code,
        legacy_cipher_hint,
        requires_captcha,
    })
}

pub fn parse_login_success(body: &str, current_twfid: &str) -> Result<String, ControlParseError> {
    if !body.contains("<Result>1</Result>") {
        return Err(ControlParseError::MissingSuccessMarker);
    }

    extract_tag(body, "TwfID")
        .map(ToOwned::to_owned)
        .or_else(|| (!current_twfid.is_empty()).then(|| current_twfid.to_string()))
        .ok_or(ControlParseError::MissingTwfId)
}

pub fn parse_resource_document(body: &str) -> Result<ResourceDocument, roxmltree::Error> {
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

fn extract_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = body.find(&start_tag)? + start_tag.len();
    let end = body[start..].find(&end_tag)? + start;
    Some(body[start..end].trim())
}

fn extract_nested_tag<'a>(body: &'a str, parent: &str, child: &str) -> Option<&'a str> {
    let parent_body = extract_tag(body, parent)?;
    extract_tag(parent_body, child)
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
