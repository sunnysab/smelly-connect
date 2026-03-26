use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use tokio::io::AsyncReadExt;
#[cfg(any(test, debug_assertions))]
use tokio::io::AsyncWriteExt;
#[cfg(not(any(test, debug_assertions)))]
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
    pool: PoolSummary,
}

#[derive(Debug, Deserialize)]
struct RuntimeSnapshot {
    total: ProtocolStats,
    http: ProtocolStats,
    socks5: ProtocolStats,
}

#[derive(Debug, Deserialize)]
struct PoolSummary {
    total_nodes: usize,
    selectable_nodes: usize,
    ready_nodes: usize,
    suspect_nodes: usize,
    open_nodes: usize,
    half_open_nodes: usize,
    connecting_nodes: usize,
    configured_nodes: usize,
}

#[derive(Debug, Deserialize)]
struct ProtocolStats {
    current_connections: u64,
    total_connections: u64,
    client_to_upstream_bytes: u64,
    upstream_to_client_bytes: u64,
}

pub async fn run_status() -> Result<(), String> {
    let output = run_status_with_config("config.toml").await?;
    println!("{output}");
    Ok(())
}

pub async fn run_status_with_config(config_path: impl AsRef<Path>) -> Result<String, String> {
    let config = crate::config::load(config_path)?;
    if !config.management.enabled {
        return Err("management API is disabled in config".to_string());
    }
    run_status_from_listen(&config.management.listen).await
}

#[cfg(any(test, debug_assertions))]
pub async fn run_status_for_test(
    listen: &str,
    health_json: &str,
    stats_json: &str,
) -> Result<String, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let health_body = health_json.to_string();
    let stats_body = stats_json.to_string();
    tokio::spawn(async move {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = vec![0_u8; 1024];
            let Ok(n) = stream.read(&mut request).await else {
                return;
            };
            let request = String::from_utf8_lossy(&request[..n]);
            let body = if request.starts_with("GET /healthz ") {
                &health_body
            } else {
                &stats_body
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    run_status_from_listen_with_label(&addr.to_string(), listen).await
}

async fn run_status_from_listen(listen: &str) -> Result<String, String> {
    run_status_from_listen_with_label(listen, listen).await
}

async fn run_status_from_listen_with_label(
    connect_target: &str,
    display_target: &str,
) -> Result<String, String> {
    let connect_target = normalize_connect_target(connect_target);
    let health: HealthResponse = fetch_json(&connect_target, "/healthz").await?;
    let stats: RuntimeSnapshot = fetch_json(&connect_target, "/stats").await?;
    Ok(format_status(display_target, health, stats))
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
    T: for<'de> Deserialize<'de>,
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

fn format_status(listen: &str, health: HealthResponse, stats: RuntimeSnapshot) -> String {
    let pool = health.pool;
    [
        format!("management={listen}"),
        format!("status={}", health.status),
        format!(
            "pool total={} selectable={} ready={} suspect={} open={} half_open={} connecting={} configured={}",
            pool.total_nodes,
            pool.selectable_nodes,
            pool.ready_nodes,
            pool.suspect_nodes,
            pool.open_nodes,
            pool.half_open_nodes,
            pool.connecting_nodes,
            pool.configured_nodes
        ),
        format_protocol("total", &stats.total),
        format_protocol("http", &stats.http),
        format_protocol("socks5", &stats.socks5),
    ]
    .join("\n")
}

fn format_protocol(name: &str, stats: &ProtocolStats) -> String {
    format!(
        "{name} current={} total={} c2u={} u2c={}",
        stats.current_connections,
        stats.total_connections,
        stats.client_to_upstream_bytes,
        stats.upstream_to_client_bytes
    )
}
