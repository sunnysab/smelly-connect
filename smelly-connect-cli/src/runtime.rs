#[cfg(any(test, debug_assertions))]
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::pool::PoolSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyProtocol {
    Http,
    Socks5,
}

#[derive(Clone, Default)]
pub struct RuntimeStats {
    http: ProtocolStats,
    socks5: ProtocolStats,
}

impl RuntimeStats {
    pub fn open_connection(&self, protocol: ProxyProtocol) -> ConnectionGuard {
        self.protocol_stats(protocol).open_connection()
    }

    pub fn snapshot(&self, pool: PoolSummary) -> RuntimeSnapshot {
        let http = self.http.snapshot();
        let socks5 = self.socks5.snapshot();
        let total = ProtocolStatsSnapshot {
            current_connections: http.current_connections + socks5.current_connections,
            total_connections: http.total_connections + socks5.total_connections,
            client_to_upstream_bytes: http.client_to_upstream_bytes
                + socks5.client_to_upstream_bytes,
            upstream_to_client_bytes: http.upstream_to_client_bytes
                + socks5.upstream_to_client_bytes,
        };
        RuntimeSnapshot {
            status: pool.status,
            pool,
            total,
            http,
            socks5,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub fn seed_protocol_for_test(&self, protocol: &str, values: BTreeMap<&str, u64>) {
        let stats = match protocol {
            "http" => &self.http,
            "socks5" => &self.socks5,
            other => panic!("unsupported protocol for test seed: {other}"),
        };
        if let Some(value) = values.get("current_connections") {
            stats.current_connections.store(*value, Ordering::Relaxed);
        }
        if let Some(value) = values.get("total_connections") {
            stats.total_connections.store(*value, Ordering::Relaxed);
        }
        if let Some(value) = values.get("client_to_upstream_bytes") {
            stats
                .client_to_upstream_bytes
                .store(*value, Ordering::Relaxed);
        }
        if let Some(value) = values.get("upstream_to_client_bytes") {
            stats
                .upstream_to_client_bytes
                .store(*value, Ordering::Relaxed);
        }
    }

    fn protocol_stats(&self, protocol: ProxyProtocol) -> &ProtocolStats {
        match protocol {
            ProxyProtocol::Http => &self.http,
            ProxyProtocol::Socks5 => &self.socks5,
        }
    }
}

#[derive(Clone, Default)]
struct ProtocolStats {
    current_connections: Arc<AtomicU64>,
    total_connections: Arc<AtomicU64>,
    client_to_upstream_bytes: Arc<AtomicU64>,
    upstream_to_client_bytes: Arc<AtomicU64>,
}

impl ProtocolStats {
    fn open_connection(&self) -> ConnectionGuard {
        self.current_connections.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard {
            stats: self.clone(),
            closed: false,
        }
    }

    fn snapshot(&self) -> ProtocolStatsSnapshot {
        ProtocolStatsSnapshot {
            current_connections: self.current_connections.load(Ordering::Relaxed),
            total_connections: self.total_connections.load(Ordering::Relaxed),
            client_to_upstream_bytes: self.client_to_upstream_bytes.load(Ordering::Relaxed),
            upstream_to_client_bytes: self.upstream_to_client_bytes.load(Ordering::Relaxed),
        }
    }
}

pub struct ConnectionGuard {
    stats: ProtocolStats,
    closed: bool,
}

impl ConnectionGuard {
    pub fn add_client_to_upstream_bytes(&self, bytes: u64) {
        self.stats
            .client_to_upstream_bytes
            .fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_upstream_to_client_bytes(&self, bytes: u64) {
        self.stats
            .upstream_to_client_bytes
            .fetch_add(bytes, Ordering::Relaxed);
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if !self.closed {
            self.stats
                .current_connections
                .fetch_sub(1, Ordering::Relaxed);
            self.closed = true;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolStatsSnapshot {
    pub current_connections: u64,
    pub total_connections: u64,
    pub client_to_upstream_bytes: u64,
    pub upstream_to_client_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub status: crate::pool::PoolHealthStatus,
    pub pool: PoolSummary,
    pub total: ProtocolStatsSnapshot,
    pub http: ProtocolStatsSnapshot,
    pub socks5: ProtocolStatsSnapshot,
}
