use crate::config::AppConfig;
use crate::error::CliError;
use crate::pool::RoutesSnapshot;

use super::fetch_management_json;

pub async fn run_routes(config: &AppConfig) -> Result<String, CliError> {
    if !config.management.enabled {
        return Err(CliError::Command(
            "management API is disabled in config".to_string(),
        ));
    }
    let routes: RoutesSnapshot =
        fetch_management_json(&config.management.listen, "/routes").await?;
    Ok(format_routes(&config.management.listen, routes))
}

fn format_routes(listen: &str, routes: RoutesSnapshot) -> String {
    let mut lines = vec![
        format!("management={listen}"),
        format!("total_nodes={}", routes.total_nodes),
    ];
    for node in routes.nodes {
        lines.push(format!("account={} state={}", node.name, node.state));
        match node.routes {
            Some(route_set) => {
                for rule in route_set.domain_rules {
                    lines.push(format!(
                        "remote domain {} ports={}-{} protocol={}",
                        rule.domain, rule.port_min, rule.port_max, rule.protocol
                    ));
                }
                for rule in route_set.ip_rules {
                    lines.push(format!(
                        "remote ip {}-{} ports={}-{} protocol={}",
                        rule.ip_min, rule.ip_max, rule.port_min, rule.port_max, rule.protocol
                    ));
                }
                for dns in route_set.static_dns {
                    lines.push(format!("remote dns {}={}", dns.host, dns.ip));
                }
            }
            None => lines.push("routes unavailable".to_string()),
        }
        if let Some(route_set) = node.local_routes {
            for rule in route_set.domain_rules {
                lines.push(format!(
                    "local domain {} ports={}-{} protocol={}",
                    rule.domain, rule.port_min, rule.port_max, rule.protocol
                ));
            }
            for rule in route_set.ip_rules {
                lines.push(format!(
                    "local ip {}-{} ports={}-{} protocol={}",
                    rule.ip_min, rule.ip_max, rule.port_min, rule.port_max, rule.protocol
                ));
            }
        }
    }
    lines.join("\n")
}
