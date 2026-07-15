use serde::Deserialize;

use crate::config::AppConfig;
use crate::error::CliError;

use super::fetch_management_json;

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

pub async fn run_status(
    config: &AppConfig,
    management_api: Option<&str>,
) -> Result<String, CliError> {
    if !config.management.enabled {
        return Err(CliError::Command(
            "management API is disabled in config".to_string(),
        ));
    }
    let listen = management_api.unwrap_or(&config.management.listen);
    let health: HealthResponse = fetch_management_json(listen, "/healthz").await?;
    let stats: RuntimeSnapshot = fetch_management_json(listen, "/stats").await?;
    Ok(format_status(listen, health, stats))
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
