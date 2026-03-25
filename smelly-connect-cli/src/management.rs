use std::net::SocketAddr;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::pool::{PoolHealthStatus, PoolSnapshot, SessionPool};
use crate::runtime::RuntimeStats;

#[derive(Clone)]
struct ManagementState {
    pool: SessionPool,
    stats: RuntimeStats,
}

#[derive(Debug, Clone, Serialize)]
struct HealthResponse {
    status: PoolHealthStatus,
    pool: PoolSnapshot,
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
        .with_state(state)
}

async fn health(State(state): State<ManagementState>) -> Json<HealthResponse> {
    let pool = state.pool.snapshot().await;
    Json(HealthResponse {
        status: pool.status,
        pool,
    })
}

async fn stats_snapshot(
    State(state): State<ManagementState>,
) -> Json<crate::runtime::RuntimeSnapshot> {
    let pool = state.pool.snapshot().await;
    Json(state.stats.snapshot(pool))
}

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
