use crate::pool::RoutesSnapshot;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use tokio::io::AsyncReadExt;
#[cfg(any(test, debug_assertions))]
use tokio::io::AsyncWriteExt;
#[cfg(not(any(test, debug_assertions)))]
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub async fn run_routes() -> Result<(), String> {
    let output = run_routes_with_config("config.toml").await?;
    println!("{output}");
    Ok(())
}

pub async fn run_routes_with_config(config_path: impl AsRef<Path>) -> Result<String, String> {
    let config = crate::config::load(config_path)?;
    if !config.management.enabled {
        return Err("management API is disabled in config".to_string());
    }
    run_routes_from_listen(&config.management.listen).await
}

#[cfg(any(test, debug_assertions))]
pub async fn run_routes_for_test(listen: &str, routes_json: &str) -> Result<String, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let routes_body = routes_json.to_string();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = vec![0_u8; 1024];
        let Ok(_n) = stream.read(&mut request).await else {
            return;
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
            routes_body.len(),
            routes_body
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
    run_routes_from_listen_with_label(&addr.to_string(), listen).await
}

async fn run_routes_from_listen(listen: &str) -> Result<String, String> {
    run_routes_from_listen_with_label(listen, listen).await
}

async fn run_routes_from_listen_with_label(
    connect_target: &str,
    display_target: &str,
) -> Result<String, String> {
    let connect_target = normalize_connect_target(connect_target);
    let routes: RoutesSnapshot = fetch_json(&connect_target, "/routes").await?;
    Ok(format_routes(display_target, routes))
}

fn normalize_connect_target(target: &str) -> String {
    let Ok(addr) = target.parse::<SocketAddr>() else {
        return target.to_string();
    };
    if !addr.ip().is_unspecified() {
        return addr.to_string();
    }
    let loopback_ip = match addr.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    SocketAddr::new(loopback_ip, addr.port()).to_string()
}

async fn fetch_json<T>(target: &str, path: &str) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let mut client = TcpStream::connect(target)
        .await
        .map_err(|err| format!("management connect failed: {err}"))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    client
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("management request failed: {err}"))?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| format!("management read failed: {err}"))?;
    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "invalid management response".to_string())?;
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| "missing management status line".to_string())?;
    if !status_line.contains(" 200 ") {
        return Err(format!("management request failed: {status_line}"));
    }
    serde_json::from_str(body).map_err(|err| format!("invalid management json: {err}"))
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
                        "domain {} ports={}-{} protocol={}",
                        rule.domain, rule.port_min, rule.port_max, rule.protocol
                    ));
                }
                for rule in route_set.ip_rules {
                    lines.push(format!(
                        "ip {}-{} ports={}-{} protocol={}",
                        rule.ip_min, rule.ip_max, rule.port_min, rule.port_max, rule.protocol
                    ));
                }
                for dns in route_set.static_dns {
                    lines.push(format!("dns {}={}", dns.host, dns.ip));
                }
            }
            None => lines.push("routes unavailable".to_string()),
        }
    }
    lines.join("\n")
}
