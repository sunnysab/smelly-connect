use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::watch;

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
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|err| err.to_string())?;
    let local_addr = listener.local_addr().map_err(|err| err.to_string())?;
    tracing::info!(listen = %local_addr, "management api listening");
    axum::serve(listener, router(pool, runtime_stats))
        .with_graceful_shutdown(async move {
            if *shutdown.borrow() {
                return;
            }
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|err| err.to_string())
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
        .route("/routes", get(routes))
        .with_state(state)
}

async fn health(State(state): State<ManagementState>) -> Json<HealthResponse> {
    let mut pool = state.pool.summary().await;
    let status = state.stats.effective_status(pool.status);
    pool.status = status;
    Json(HealthResponse { status, pool })
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

async fn routes(State(state): State<ManagementState>) -> Json<crate::pool::RoutesSnapshot> {
    Json(state.pool.routes_snapshot().await)
}
