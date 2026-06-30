use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::error::CliError;

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
    active_nodes: usize,
    connecting_nodes: usize,
    idle_nodes: usize,
    dead_nodes: usize,
    disabled_nodes: usize,
    #[serde(default)]
    total_reconnections: u64,
}

#[derive(Debug, Deserialize)]
struct ProtocolStats {
    current_connections: u64,
    total_connections: u64,
    client_to_upstream_bytes: u64,
    upstream_to_client_bytes: u64,
    #[serde(default)]
    service_unavailable_no_ready_session: u64,
    #[serde(default)]
    service_unavailable_over_capacity: u64,
}

pub async fn run_status() -> Result<(), String> {
    let output = run_status_with_config("config.toml").await?;
    println!("{output}");
    Ok(())
}

pub async fn run_status_with_config(config_path: impl AsRef<Path>) -> Result<String, String> {
    run_status_with_config_and_management_api(config_path, None).await
}

pub async fn run_status_with_config_and_management_api(
    config_path: impl AsRef<Path>,
    management_api: Option<String>,
) -> Result<String, String> {
    run_status_with_config_and_management_api_typed(config_path, management_api)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_status_with_config_and_management_api_typed(
    config_path: impl AsRef<Path>,
    management_api: Option<String>,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let listen = management_api.unwrap_or(config.management.listen);
    run_status_from_configured_target(config.management.enabled, &listen).await
}

async fn run_status_from_configured_target(
    management_enabled: bool,
    listen: &str,
) -> Result<String, CliError> {
    if !management_enabled {
        return Err(CliError::Command(
            "management API is disabled in config".to_string(),
        ));
    }
    run_status_from_listen_typed(listen).await
}

pub async fn run_status_with_config_typed(
    config_path: impl AsRef<Path>,
) -> Result<String, CliError> {
    run_status_with_config_and_management_api_typed(config_path, None).await
}

async fn run_status_from_listen_typed(listen: &str) -> Result<String, CliError> {
    run_status_from_listen_with_label(listen, listen).await
}

async fn run_status_from_listen_with_label(
    connect_target: &str,
    display_target: &str,
) -> Result<String, CliError> {
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

async fn fetch_json<T>(target: &str, path: &str) -> Result<T, CliError>
where
    T: for<'de> Deserialize<'de>,
{
    let mut client = TcpStream::connect(target)
        .await
        .map_err(|err| CliError::Command(format!("management connect failed: {err}")))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    client
        .write_all(request.as_bytes())
        .await
        .map_err(|err| CliError::Command(format!("management request failed: {err}")))?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| CliError::Command(format!("management read failed: {err}")))?;
    let response = String::from_utf8(response).map_err(|err| CliError::Command(err.to_string()))?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| CliError::Command("invalid management response".to_string()))?;
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| CliError::Command("missing management status line".to_string()))?;
    if !status_line.contains(" 200 ") {
        return Err(CliError::Command(format!(
            "management request failed: {status_line}"
        )));
    }
    serde_json::from_str(body)
        .map_err(|err| CliError::Command(format!("invalid management json: {err}")))
}

fn format_status(listen: &str, health: HealthResponse, stats: RuntimeSnapshot) -> String {
    let pool = health.pool;
    [
        format!("management={listen}"),
        format!("status={}", health.status),
        format!(
            "pool total={} selectable={} active={} connecting={} idle={} dead={} disabled={} reconnects={}",
            pool.total_nodes,
            pool.selectable_nodes,
            pool.active_nodes,
            pool.connecting_nodes,
            pool.idle_nodes,
            pool.dead_nodes,
            pool.disabled_nodes,
            pool.total_reconnections,
        ),
        format_protocol("total", &stats.total),
        format_protocol("http", &stats.http),
        format_protocol("socks5", &stats.socks5),
    ]
    .join("\n")
}

fn format_protocol(name: &str, stats: &ProtocolStats) -> String {
    format!(
        "{name} current={} total={} c2u={} u2c={} svc503_no_ready={} svc503_over_capacity={}",
        stats.current_connections,
        stats.total_connections,
        format_bytes(stats.client_to_upstream_bytes),
        format_bytes(stats.upstream_to_client_bytes),
        stats.service_unavailable_no_ready_session,
        stats.service_unavailable_over_capacity,
    )
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64;
    let mut unit_index = 0;
    while value >= 1000.0 && unit_index < UNITS.len() - 1 {
        value /= 1000.0;
        unit_index += 1;
    }
    format!("{value:.1} {}", UNITS[unit_index])
}
