#[cfg(any(test, debug_assertions))]
use std::net::SocketAddr;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
#[cfg(any(test, debug_assertions))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
#[cfg(any(test, debug_assertions))]
use tokio::net::TcpStream;

use crate::pool::{PoolHealthStatus, PoolSummary, SessionPool};
use crate::runtime::RuntimeStats;

#[derive(Clone)]
struct ManagementState {
    pool: SessionPool,
    stats: RuntimeStats,
}

#[derive(Debug, Clone, Serialize)]
struct HealthResponse {
    status: PoolHealthStatus,
    pool: PoolSummary,
}

#[derive(Debug, Clone, Serialize)]
struct NodesResponse {
    total_nodes: usize,
    nodes: Vec<crate::pool::AccountNodeSnapshot>,
}

pub async fn serve_management(
    listen: String,
    pool: SessionPool,
    runtime_stats: RuntimeStats,
) -> Result<(), String> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|err| err.to_string())?;
    let local_addr = listener.local_addr().map_err(|err| err.to_string())?;
    tracing::info!(listen = %local_addr, "management api listening");
    axum::serve(listener, router(pool, runtime_stats))
        .await
        .map_err(|err| err.to_string())
}

#[cfg(any(test, debug_assertions))]
pub async fn fetch_json_for_test(
    pool: SessionPool,
    runtime_stats: RuntimeStats,
    path: &str,
) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let app = router(pool, runtime_stats);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    request_json(addr, path).await
}

fn router(pool: SessionPool, runtime_stats: RuntimeStats) -> Router {
    let state = ManagementState {
        pool,
        stats: runtime_stats,
    };
    Router::new()
        .route("/healthz", get(health))
        .route("/stats", get(stats_snapshot))
        .route("/nodes", get(nodes))
        .with_state(state)
}

async fn health(State(state): State<ManagementState>) -> Json<HealthResponse> {
    let pool = state.pool.summary().await;
    Json(HealthResponse {
        status: pool.status,
        pool,
    })
}

async fn stats_snapshot(
    State(state): State<ManagementState>,
) -> Json<crate::runtime::RuntimeSnapshot> {
    let pool = state.pool.summary().await;
    Json(state.stats.snapshot(pool))
}

async fn nodes(State(state): State<ManagementState>) -> Json<NodesResponse> {
    let snapshot = state.pool.snapshot().await;
    Json(NodesResponse {
        total_nodes: snapshot.summary.total_nodes,
        nodes: snapshot.nodes,
    })
}

#[cfg(any(test, debug_assertions))]
async fn request_json(addr: SocketAddr, path: &str) -> Result<String, String> {
    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    client
        .write_all(request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let response = String::from_utf8(response).map_err(|err| err.to_string())?;
    response
        .split("\r\n\r\n")
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| "missing management response body".to_string())
}
