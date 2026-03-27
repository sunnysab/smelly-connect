use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use smelly_connect::{CaptchaError, CaptchaHandler, EasyConnectClient, LocalRouteOverrides, Session};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::{AccountConfig, AppConfig};

#[derive(Clone)]
pub struct PooledSession {
    account_name: String,
    session: Option<Session>,
}

impl PooledSession {
    pub fn account_name(&self) -> &str {
        &self.account_name
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }
}

impl std::fmt::Debug for PooledSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledSession")
            .field("account_name", &self.account_name)
            .field("has_session", &self.session.is_some())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct AccountFailure {
    pub message: String,
}

#[derive(Clone)]
pub enum AccountState {
    Configured(AccountConfig),
    Connecting,
    Ready(Box<PooledSession>),
    Suspect(Box<PooledSession>),
    Open(AccountFailure),
    HalfOpen(AccountConfig),
}

#[derive(Clone)]
struct AccountNode {
    name: String,
    account: AccountConfig,
    state: AccountState,
    #[allow(dead_code)]
    flaky_retry: bool,
    consecutive_failures: u32,
    failure_threshold: u32,
    current_backoff: Duration,
    backoff_base: Duration,
    backoff_max: Duration,
    open_until: Option<Instant>,
}

#[derive(Default)]
struct PoolState {
    nodes: Vec<AccountNode>,
    cursor: usize,
}

#[derive(Clone)]
pub struct SessionPool {
    inner: Arc<Mutex<PoolState>>,
    #[cfg(any(test, debug_assertions))]
    retry_delay: Duration,
    connect_timeout: Duration,
    local_route_overrides: LocalRouteOverrides,
    allow_all_routes: bool,
    server: Option<String>,
    allow_request_triggered_probe: bool,
}

#[derive(Debug, Clone)]
pub struct PoolError {
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolStartupMode {
    RequireReady,
    AllowEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRaceResult {
    pub successes: usize,
    pub fast_failures: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolHealthStatus {
    Healthy,
    Recovering,
    Down,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountNodeSnapshot {
    pub name: String,
    pub state: String,
    pub consecutive_failures: u32,
    pub failure_threshold: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoolSummary {
    pub status: PoolHealthStatus,
    pub total_nodes: usize,
    pub selectable_nodes: usize,
    pub ready_nodes: usize,
    pub suspect_nodes: usize,
    pub open_nodes: usize,
    pub half_open_nodes: usize,
    pub connecting_nodes: usize,
    pub configured_nodes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoolSnapshot {
    #[serde(flatten)]
    pub summary: PoolSummary,
    pub nodes: Vec<AccountNodeSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRouteSnapshot {
    pub domain: String,
    pub port_min: u16,
    pub port_max: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRouteSnapshot {
    pub ip_min: String,
    pub ip_max: String,
    pub port_min: u16,
    pub port_max: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticDnsSnapshot {
    pub host: String,
    pub ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteSetSnapshot {
    pub domain_rules: Vec<DomainRouteSnapshot>,
    pub ip_rules: Vec<IpRouteSnapshot>,
    pub static_dns: Vec<StaticDnsSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRoutesSnapshot {
    pub name: String,
    pub state: String,
    pub routes: Option<RouteSetSnapshot>,
    pub local_routes: Option<RouteSetSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutesSnapshot {
    pub total_nodes: usize,
    pub nodes: Vec<AccountRoutesSnapshot>,
}

impl PoolError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for PoolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PoolError {}

impl SessionPool {
    pub async fn from_config_allow_empty(cfg: &AppConfig) -> Result<Self, PoolError> {
        Self::from_config_with_startup_mode(cfg, PoolStartupMode::AllowEmpty).await
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_test_accounts(total: usize, prewarm: usize) -> Self {
        let mut nodes = Vec::new();
        for idx in 0..total {
            let name = format!("acct-{:02}", idx + 1);
            let state = if idx < prewarm {
                AccountState::Ready(
                    PooledSession {
                        account_name: name.clone(),
                        session: None,
                    }
                    .into(),
                )
            } else {
                AccountState::Configured(AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                })
            };
            nodes.push(AccountNode {
                name: name.clone(),
                account: AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                },
                state,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_named_ready_accounts<const N: usize>(names: [&str; N]) -> Self {
        let nodes = names
            .into_iter()
            .map(|name| AccountNode {
                name: name.to_string(),
                account: AccountConfig {
                    name: name.to_string(),
                    username: name.to_string(),
                    password: "pass".to_string(),
                },
                state: AccountState::Ready(
                    PooledSession {
                        account_name: name.to_string(),
                        session: None,
                    }
                    .into(),
                ),
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_named_ready_live_accounts<const N: usize>(
        entries: [(&str, &str, std::net::Ipv4Addr); N],
    ) -> Self {
        let nodes = entries
            .into_iter()
            .map(|(account_name, host, ip)| AccountNode {
                name: account_name.to_string(),
                account: AccountConfig {
                    name: account_name.to_string(),
                    username: account_name.to_string(),
                    password: "pass".to_string(),
                },
                state: AccountState::Ready(
                    PooledSession {
                        account_name: account_name.to_string(),
                        session: Some(smelly_connect::session::tests::session_with_domain_match(
                            host, ip,
                        )),
                    }
                    .into(),
                ),
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_live_sessions_for_test(entries: Vec<(&str, Session)>) -> Self {
        let nodes = entries
            .into_iter()
            .map(|(account_name, session)| AccountNode {
                name: account_name.to_string(),
                account: AccountConfig {
                    name: account_name.to_string(),
                    username: account_name.to_string(),
                    password: "pass".to_string(),
                },
                state: AccountState::Ready(
                    PooledSession {
                        account_name: account_name.to_string(),
                        session: Some(session),
                    }
                    .into(),
                ),
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_test_outcomes<const N: usize>(
        outcomes: [Result<&str, &str>; N],
        prewarm: usize,
    ) -> Self {
        let mut nodes = Vec::new();
        for (idx, outcome) in outcomes.into_iter().enumerate() {
            let (name, state) = match outcome {
                Ok(name) if idx < prewarm => (
                    name.to_string(),
                    AccountState::Ready(
                        PooledSession {
                            account_name: name.to_string(),
                            session: None,
                        }
                        .into(),
                    ),
                ),
                Ok(name) => (
                    name.to_string(),
                    AccountState::Configured(AccountConfig {
                        name: name.to_string(),
                        username: name.to_string(),
                        password: "pass".to_string(),
                    }),
                ),
                Err(message) => (
                    format!("failed-{idx}"),
                    AccountState::Open(AccountFailure {
                        message: message.to_string(),
                    }),
                ),
            };
            nodes.push(AccountNode {
                name: name.clone(),
                account: AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                },
                state,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_failed_accounts(total: usize) -> Self {
        let mut nodes = Vec::new();
        for idx in 0..total {
            let name = format!("failed-{:02}", idx + 1);
            nodes.push(AccountNode {
                name: name.clone(),
                account: AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                },
                state: AccountState::Open(AccountFailure {
                    message: "not ready".to_string(),
                }),
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_flaky_account_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    name: "acct-01".to_string(),
                    account: AccountConfig {
                        name: "acct-01".to_string(),
                        username: "acct-01".to_string(),
                        password: "pass".to_string(),
                    },
                    state: AccountState::Ready(
                        PooledSession {
                            account_name: "acct-01".to_string(),
                            session: None,
                        }
                        .into(),
                    ),
                    flaky_retry: true,
                    consecutive_failures: 0,
                    failure_threshold: 3,
                    current_backoff: Duration::from_secs(30),
                    backoff_base: Duration::from_secs(30),
                    backoff_max: Duration::from_secs(600),
                    open_until: None,
                }],
                cursor: 0,
            })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_millis(100),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    pub async fn from_config(cfg: &AppConfig) -> Result<Self, PoolError> {
        Self::from_config_with_startup_mode(cfg, PoolStartupMode::RequireReady).await
    }

    async fn from_config_with_startup_mode(
        cfg: &AppConfig,
        startup_mode: PoolStartupMode,
    ) -> Result<Self, PoolError> {
        tracing::info!(
            accounts = cfg.accounts.len(),
            prewarm = cfg.pool.prewarm,
            "pool prewarm start"
        );
        let mut nodes = Vec::new();
        for account in &cfg.accounts {
            nodes.push(AccountNode {
                name: account.name.clone(),
                account: account.clone(),
                state: AccountState::Configured(account.clone()),
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: cfg.pool.failure_threshold,
                current_backoff: Duration::from_secs(cfg.pool.backoff_base_secs),
                backoff_base: Duration::from_secs(cfg.pool.backoff_base_secs),
                backoff_max: Duration::from_secs(cfg.pool.backoff_max_secs),
                open_until: None,
            });
        }

        let pool = Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            #[cfg(any(test, debug_assertions))]
            retry_delay: Duration::from_secs(cfg.pool.healthcheck_interval_secs.max(1)),
            connect_timeout: Duration::from_secs(cfg.pool.connect_timeout_secs.max(1)),
            local_route_overrides: build_local_route_overrides(&cfg.routing)?,
            allow_all_routes: cfg.routing.allow_all,
            server: Some(cfg.vpn.server.clone()),
            allow_request_triggered_probe: cfg.pool.allow_request_triggered_probe,
        };

        pool.prewarm(cfg.pool.prewarm).await;
        let ready = pool.ready_count().await;
        tracing::info!(
            configured = cfg.accounts.len(),
            ready,
            "pool startup summary"
        );
        if ready == 0 {
            match startup_mode {
                PoolStartupMode::RequireReady => {
                    tracing::error!("no ready session after prewarm");
                    return Err(PoolError::new("no ready session after prewarm"));
                }
                PoolStartupMode::AllowEmpty => {
                    tracing::warn!("starting with no ready session after prewarm");
                }
            }
        }
        Ok(pool)
    }

    pub async fn ready_count(&self) -> usize {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .filter(|node| matches!(node.state, AccountState::Ready(_)))
            .count()
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn state_summary_for_test(&self) -> String {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .map(|node| {
                let label = state_label(&node.state);
                format!("{}:{label}", node.name)
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    pub async fn snapshot(&self) -> PoolSnapshot {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        let summary = build_pool_summary(&state);
        let mut nodes = Vec::with_capacity(state.nodes.len());

        for node in &state.nodes {
            nodes.push(AccountNodeSnapshot {
                name: node.name.clone(),
                state: state_label(&node.state).to_ascii_lowercase(),
                consecutive_failures: node.consecutive_failures,
                failure_threshold: node.failure_threshold,
            });
        }

        PoolSnapshot { summary, nodes }
    }

    pub async fn summary(&self) -> PoolSummary {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        build_pool_summary(&state)
    }

    pub async fn routes_snapshot(&self) -> RoutesSnapshot {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        let mut nodes = Vec::with_capacity(state.nodes.len());

        for node in &state.nodes {
            let routes = match &node.state {
                AccountState::Ready(session) | AccountState::Suspect(session) => {
                    session.session().map(build_route_set_snapshot)
                }
                _ => None,
            };
            let local_routes = match &node.state {
                AccountState::Ready(session) | AccountState::Suspect(session) => session
                    .session()
                    .map(build_local_route_set_snapshot)
                    .filter(|routes| {
                        !routes.domain_rules.is_empty()
                            || !routes.ip_rules.is_empty()
                            || !routes.static_dns.is_empty()
                    }),
                _ => None,
            };
            nodes.push(AccountRoutesSnapshot {
                name: node.name.clone(),
                state: state_label(&node.state).to_ascii_lowercase(),
                routes,
                local_routes,
            });
        }

        RoutesSnapshot {
            total_nodes: nodes.len(),
            nodes,
        }
    }

    pub async fn report_live_session_failure(&self, account_name: &str, error: impl Into<String>) {
        let error = error.into();
        let mut state = self.inner.lock().await;
        if let Some(node) = state.nodes.iter_mut().find(|node| node.name == account_name)
            && matches!(node.state, AccountState::Ready(_) | AccountState::Suspect(_))
        {
            node.consecutive_failures = node.failure_threshold;
            open_node(node, error.clone());
            tracing::warn!(
                account = %account_name,
                error = %error,
                "live session marked open after proxy failure"
            );
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn has_selectable_nodes_for_test(&self) -> bool {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        state.nodes.iter().any(|node| {
            matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_)
            )
        })
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_mixed_state_pool_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![
                    AccountNode {
                        name: "ready-01".to_string(),
                        account: AccountConfig {
                            name: "ready-01".to_string(),
                            username: "ready-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::Ready(
                            PooledSession {
                                account_name: "ready-01".to_string(),
                                session: None,
                            }
                            .into(),
                        ),
                        flaky_retry: false,
                        consecutive_failures: 0,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: None,
                    },
                    AccountNode {
                        name: "suspect-01".to_string(),
                        account: AccountConfig {
                            name: "suspect-01".to_string(),
                            username: "suspect-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::Suspect(
                            PooledSession {
                                account_name: "suspect-01".to_string(),
                                session: None,
                            }
                            .into(),
                        ),
                        flaky_retry: false,
                        consecutive_failures: 1,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: None,
                    },
                    AccountNode {
                        name: "open-01".to_string(),
                        account: AccountConfig {
                            name: "open-01".to_string(),
                            username: "open-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::Open(AccountFailure {
                            message: "open".to_string(),
                        }),
                        flaky_retry: false,
                        consecutive_failures: 3,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: Some(Instant::now() + Duration::from_secs(30)),
                    },
                    AccountNode {
                        name: "half-open-01".to_string(),
                        account: AccountConfig {
                            name: "half-open-01".to_string(),
                            username: "half-open-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::HalfOpen(AccountConfig {
                            name: "half-open-01".to_string(),
                            username: "half-open-01".to_string(),
                            password: "pass".to_string(),
                        }),
                        flaky_retry: false,
                        consecutive_failures: 3,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: None,
                    },
                ],
                cursor: 0,
            })),
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn from_exhausted_pool_for_test() -> Self {
        let account = AccountConfig {
            name: "acct-01".to_string(),
            username: "acct-01".to_string(),
            password: "pass".to_string(),
        };
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    name: account.name.clone(),
                    account: account.clone(),
                    state: AccountState::Open(AccountFailure {
                        message: "vpn unavailable".to_string(),
                    }),
                    flaky_retry: false,
                    consecutive_failures: 3,
                    failure_threshold: 3,
                    current_backoff: Duration::from_secs(30),
                    backoff_base: Duration::from_secs(30),
                    backoff_max: Duration::from_secs(600),
                    open_until: Some(Instant::now() + Duration::from_secs(30)),
                }],
                cursor: 0,
            })),
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            allow_all_routes: false,
            server: None,
            allow_request_triggered_probe: true,
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn collect_selected_accounts_for_test(&self, count: usize) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..count {
            match self.next_account_name().await {
                Ok(name) => out.push(name),
                Err(_) => break,
            }
        }
        out
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn current_backoff_for_test(&self) -> Duration {
        let state = self.inner.lock().await;
        state
            .nodes
            .first()
            .map(|node| node.current_backoff)
            .unwrap_or_default()
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn connect_timeout_for_test(&self) -> Duration {
        self.connect_timeout
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn try_request_triggered_probe_for_test(&self) -> Result<PooledSession, PoolError> {
        let Some((name, account)) = self.claim_request_triggered_probe().await? else {
            return Err(PoolError::new("no ready session"));
        };
        let session = PooledSession {
            account_name: name.clone(),
            session: None,
        };
        self.complete_probe_success(&name, session.clone(), account)
            .await?;
        Ok(session)
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn run_concurrent_probe_race_for_test(&self) -> ProbeRaceResult {
        let first = {
            let pool = self.clone();
            tokio::spawn(async move { pool.try_request_triggered_probe_for_test().await })
        };
        let second = {
            let pool = self.clone();
            tokio::spawn(async move { pool.try_request_triggered_probe_for_test().await })
        };

        let mut results = ProbeRaceResult {
            successes: 0,
            fast_failures: 0,
        };

        for outcome in [first.await, second.await] {
            match outcome {
                Ok(Ok(_)) => results.successes += 1,
                Ok(Err(err)) if err.to_string().contains("no ready session") => {
                    results.fast_failures += 1;
                }
                Ok(Err(err)) => panic!("unexpected probe failure: {err}"),
                Err(err) => panic!("probe task join failure: {err}"),
            }
        }

        results
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn force_probe_failure_for_test(&self) {
        let mut state = self.inner.lock().await;
        if let Some(node) = state.nodes.first_mut() {
            node.current_backoff =
                next_backoff(node.current_backoff, node.backoff_base, node.backoff_max);
            node.open_until = Some(Instant::now() + node.current_backoff);
            node.state = AccountState::Open(AccountFailure {
                message: "forced probe failure".to_string(),
            });
            let name = node.name.clone();
            let backoff = node.current_backoff;
            let account = node.account.clone();
            let inner = Arc::clone(&self.inner);
            drop(state);
            tokio::spawn(async move {
                tokio::time::sleep(backoff).await;
                let mut state = inner.lock().await;
                if let Some(node) = state.nodes.iter_mut().find(|node| node.name == name) {
                    node.state = AccountState::HalfOpen(account);
                }
            });
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn next_account_name(&self) -> Result<String, PoolError> {
        Ok(self.next_session().await?.account_name().to_string())
    }

    pub async fn next_session(&self) -> Result<PooledSession, PoolError> {
        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        let ready: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| match &node.state {
                AccountState::Ready(session) | AccountState::Suspect(session) => {
                    Some((idx, session.as_ref().clone()))
                }
                _ => None,
            })
            .collect();

        if ready.is_empty() {
            return Err(PoolError::new("no ready session"));
        }

        let pos = state.cursor % ready.len();
        state.cursor += 1;
        Ok(ready[pos].1.clone())
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn ensure_additional_capacity_for_test(&self) -> Result<(), PoolError> {
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| matches!(node.state, AccountState::Configured(_)))
        {
            node.state = AccountState::Ready(
                PooledSession {
                    account_name: node.name.clone(),
                    session: None,
                }
                .into(),
            );
            return Ok(());
        }
        Err(PoolError::new("no configurable account remaining"))
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn force_one_failure_for_test(&self) {
        self.force_failures_for_test(1).await;
    }

    #[cfg(any(test, debug_assertions))]
    pub async fn force_failures_for_test(&self, count: u32) {
        for _ in 0..count {
            let mut should_retry = None;
            {
                let mut state = self.inner.lock().await;
                if let Some(node) = state.nodes.iter_mut().find(|node| {
                    matches!(
                        node.state,
                        AccountState::Ready(_) | AccountState::Suspect(_)
                    )
                }) {
                    let name = node.name.clone();
                    let flaky_retry = node.flaky_retry;
                    node.consecutive_failures += 1;

                    let session = match std::mem::replace(
                        &mut node.state,
                        AccountState::Open(AccountFailure {
                            message: "forced failure".to_string(),
                        }),
                    ) {
                        AccountState::Ready(session) | AccountState::Suspect(session) => session,
                        other => {
                            node.state = other;
                            continue;
                        }
                    };

                    if node.consecutive_failures >= node.failure_threshold {
                        node.open_until = Some(Instant::now() + node.current_backoff);
                        tracing::warn!(
                            account = %name,
                            failures = node.consecutive_failures,
                            "node moved to open"
                        );
                        should_retry = flaky_retry.then_some(name);
                    } else {
                        node.state = AccountState::Suspect(session);
                        tracing::warn!(
                            account = %name,
                            failures = node.consecutive_failures,
                            "node marked suspect"
                        );
                    }
                }
            }

            if let Some(name) = should_retry {
                let inner = Arc::clone(&self.inner);
                let retry_delay = self.retry_delay;
                tokio::spawn(async move {
                    tracing::warn!(
                        account = %name,
                        delay_ms = retry_delay.as_millis(),
                        "retrying account after fixed-delay backoff"
                    );
                    tokio::time::sleep(retry_delay).await;
                    let mut state = inner.lock().await;
                    if let Some(node) = state.nodes.iter_mut().find(|node| node.name == name) {
                        node.state = AccountState::Ready(
                            PooledSession {
                                account_name: node.account.name.clone(),
                                session: None,
                            }
                            .into(),
                        );
                        node.consecutive_failures = 0;
                        node.open_until = None;
                    }
                });
            }
        }
    }

    pub async fn next_live_session(&self) -> Result<(String, Session), PoolError> {
        self.refresh_time_based_states().await;
        if let Some(ready) = self.next_ready_with_session().await? {
            return Ok(ready);
        }

        let _ = self.connect_one_configured().await;

        if let Some(ready) = self.next_ready_with_session().await? {
            return Ok(ready);
        }

        if let Some(probed) = self.try_request_triggered_live_probe().await? {
            return Ok(probed);
        }

        Err(PoolError::new("no ready session"))
    }

    async fn prewarm(&self, count: usize) {
        for _ in 0..count {
            let _ = self.connect_one_configured().await;
        }
    }

    async fn next_ready_with_session(&self) -> Result<Option<(String, Session)>, PoolError> {
        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        let ready: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| match &node.state {
                AccountState::Ready(session) | AccountState::Suspect(session) => session
                    .session()
                    .cloned()
                    .map(|live| (idx, session.account_name().to_string(), live)),
                _ => None,
            })
            .collect();

        if ready.is_empty() {
            return Ok(None);
        }

        let pos = state.cursor % ready.len();
        state.cursor += 1;
        let (_, account_name, session) = ready[pos].clone();
        Ok(Some((account_name, session)))
    }

    async fn connect_one_configured(&self) -> Result<(), PoolError> {
        let (name, account, server) = {
            let mut state = self.inner.lock().await;
            let Some(server) = self.server.clone() else {
                return Err(PoolError::new("real server configuration unavailable"));
            };
            let Some(idx) = state
                .nodes
                .iter_mut()
                .enumerate()
                .find(|(_, node)| matches!(node.state, AccountState::Configured(_)))
                .map(|(idx, _)| idx)
            else {
                return Err(PoolError::new("no configurable account remaining"));
            };
            let account = state.nodes[idx].account.clone();
            let name = state.nodes[idx].name.clone();
            state.nodes[idx].state = AccountState::Connecting;
            (name, account, server)
        };

        match connect_account(
            &server,
            &account,
            self.connect_timeout,
            &self.local_route_overrides,
            self.allow_all_routes,
        )
        .await
        {
            Ok(session) => {
                let mut state = self.inner.lock().await;
                if let Some(node) = state.nodes.iter_mut().find(|node| node.name == name) {
                    node.state = AccountState::Ready(
                        PooledSession {
                            account_name: account.name.clone(),
                            session: Some(session),
                        }
                        .into(),
                    );
                    tracing::info!(account = %account.name, "account ready");
                }
                Ok(())
            }
            Err(err) => {
                let mut state = self.inner.lock().await;
                if let Some(node) = state.nodes.iter_mut().find(|node| node.name == name) {
                    open_node(node, err.to_string());
                }
                tracing::warn!(account = %account.name, error = %err, "account prewarm failed");
                Err(err)
            }
        }
    }

    async fn claim_request_triggered_probe(
        &self,
    ) -> Result<Option<(String, AccountConfig)>, PoolError> {
        if !self.allow_request_triggered_probe {
            return Ok(None);
        }

        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        if state.nodes.iter().any(|node| {
            matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_) | AccountState::Connecting
            )
        }) {
            return Ok(None);
        }

        let probe_candidates: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| {
                matches!(node.state, AccountState::HalfOpen(_)).then_some(idx)
            })
            .collect();
        if probe_candidates.is_empty() {
            return Ok(None);
        }

        let pos = state.cursor % probe_candidates.len();
        state.cursor += 1;
        let idx = probe_candidates[pos];
        let node = &mut state.nodes[idx];
        let account = node.account.clone();
        let name = node.name.clone();
        node.state = AccountState::Connecting;
        node.open_until = None;
        tracing::info!(account = %name, "request-triggered recovery probe scheduled");
        Ok(Some((name, account)))
    }

    async fn try_request_triggered_live_probe(
        &self,
    ) -> Result<Option<(String, Session)>, PoolError> {
        let Some((name, account)) = self.claim_request_triggered_probe().await? else {
            return Ok(None);
        };
        let server = self
            .server
            .as_deref()
            .ok_or_else(|| PoolError::new("real server configuration unavailable"))?;

        match connect_account(
            server,
            &account,
            self.connect_timeout,
            &self.local_route_overrides,
            self.allow_all_routes,
        )
        .await
        {
            Ok(session) => {
                self.complete_probe_success(
                    &name,
                    PooledSession {
                        account_name: account.name.clone(),
                        session: Some(session.clone()),
                    },
                    account,
                )
                .await?;
                Ok(Some((name, session)))
            }
            Err(err) => {
                self.complete_probe_failure(&name, err.to_string()).await?;
                Err(err)
            }
        }
    }

    async fn complete_probe_success(
        &self,
        name: &str,
        session: PooledSession,
        account: AccountConfig,
    ) -> Result<(), PoolError> {
        let mut state = self.inner.lock().await;
        let node = state
            .nodes
            .iter_mut()
            .find(|node| node.name == name)
            .ok_or_else(|| PoolError::new(format!("probe target disappeared: {name}")))?;
        node.account = account;
        node.consecutive_failures = 0;
        node.current_backoff = node.backoff_base;
        node.open_until = None;
        node.state = AccountState::Ready(Box::new(session));
        tracing::info!(account = %name, "request-triggered recovery probe succeeded");
        Ok(())
    }

    async fn complete_probe_failure(&self, name: &str, error: String) -> Result<(), PoolError> {
        let mut state = self.inner.lock().await;
        let node = state
            .nodes
            .iter_mut()
            .find(|node| node.name == name)
            .ok_or_else(|| PoolError::new(format!("probe target disappeared: {name}")))?;
        open_node(node, error.clone());
        tracing::warn!(account = %name, error = %error, "request-triggered recovery probe failed");
        Ok(())
    }

    async fn refresh_time_based_states(&self) {
        let mut state = self.inner.lock().await;
        let now = Instant::now();
        for node in &mut state.nodes {
            if let (AccountState::Open(_), Some(open_until)) = (&node.state, node.open_until)
                && now >= open_until
            {
                node.state = AccountState::HalfOpen(node.account.clone());
                node.open_until = None;
            }
        }
    }
}

async fn connect_account(
    server: &str,
    account: &AccountConfig,
    timeout: Duration,
    local_route_overrides: &LocalRouteOverrides,
    allow_all_routes: bool,
) -> Result<Session, PoolError> {
    let client = EasyConnectClient::builder(server.to_string())
        .credentials(account.username.clone(), account.password.clone())
        .with_captcha_handler(CaptchaHandler::from_async(|_, _| async move {
            Err(CaptchaError::new(
                "captcha callback not configured for smelly-connect-cli",
            ))
        }))
        .build()
        .map_err(|err| PoolError::new(format!("{err:?}")))?;

    let session = tokio::time::timeout(timeout, client.connect())
        .await
        .map_err(|_| PoolError::new("session connect timeout"))?
        .map_err(|err| PoolError::new(format!("{err:?}")))?;
    Ok(session
        .with_local_route_overrides(local_route_overrides.clone())
        .with_allow_all_routes(allow_all_routes))
}

fn next_backoff(current: Duration, base: Duration, max: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled < base {
        base
    } else if doubled > max {
        max
    } else {
        doubled
    }
}

fn state_label(state: &AccountState) -> &'static str {
    match state {
        AccountState::Configured(_) => "Configured",
        AccountState::Connecting => "Connecting",
        AccountState::Ready(_) => "Ready",
        AccountState::Suspect(_) => "Suspect",
        AccountState::Open(_) => "Open",
        AccountState::HalfOpen(_) => "HalfOpen",
    }
}

fn build_route_set_snapshot(session: &Session) -> RouteSetSnapshot {
    let resources = session.resources();

    let mut domain_rules = resources
        .domain_rules
        .iter()
        .map(|(domain, rule)| DomainRouteSnapshot {
            domain: domain.clone(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.clone(),
        })
        .collect::<Vec<_>>();
    domain_rules.sort_by(|a, b| a.domain.cmp(&b.domain));

    let mut ip_rules = resources
        .ip_rules
        .iter()
        .map(|rule| IpRouteSnapshot {
            ip_min: rule.ip_min.to_string(),
            ip_max: rule.ip_max.to_string(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.clone(),
        })
        .collect::<Vec<_>>();
    ip_rules.sort_by(|a, b| {
        (&a.ip_min, &a.ip_max, a.port_min, a.port_max, &a.protocol).cmp(&(
            &b.ip_min,
            &b.ip_max,
            b.port_min,
            b.port_max,
            &b.protocol,
        ))
    });

    let mut static_dns = resources
        .static_dns
        .iter()
        .map(|(host, ip)| StaticDnsSnapshot {
            host: host.clone(),
            ip: ip.to_string(),
        })
        .collect::<Vec<_>>();
    static_dns.sort_by(|a, b| a.host.cmp(&b.host));

    RouteSetSnapshot {
        domain_rules,
        ip_rules,
        static_dns,
    }
}

fn build_local_route_set_snapshot(session: &Session) -> RouteSetSnapshot {
    let local = session.local_route_overrides();

    let mut domain_rules = local
        .domain_rules()
        .iter()
        .map(|(domain, rule)| DomainRouteSnapshot {
            domain: domain.clone(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.clone(),
        })
        .collect::<Vec<_>>();
    domain_rules.sort_by(|a, b| a.domain.cmp(&b.domain));

    let mut ip_rules = local
        .ip_rules()
        .iter()
        .map(|rule| IpRouteSnapshot {
            ip_min: rule.ip_min.to_string(),
            ip_max: rule.ip_max.to_string(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.clone(),
        })
        .collect::<Vec<_>>();
    ip_rules.sort_by(|a, b| {
        (&a.ip_min, &a.ip_max, a.port_min, a.port_max, &a.protocol).cmp(&(
            &b.ip_min,
            &b.ip_max,
            b.port_min,
            b.port_max,
            &b.protocol,
        ))
    });

    RouteSetSnapshot {
        domain_rules,
        ip_rules,
        static_dns: vec![],
    }
}

fn build_local_route_overrides(
    config: &crate::config::RoutingConfig,
) -> Result<LocalRouteOverrides, PoolError> {
    let mut domain_rules = HashMap::new();
    for rule in &config.domain_rules {
        let domain = normalize_override_domain(&rule.domain);
        if domain.is_empty() {
            return Err(PoolError::new("routing.domain_rules contains empty domain"));
        }
        domain_rules.insert(
            domain,
            smelly_connect::resource::DomainRule {
                port_min: rule.port_min,
                port_max: rule.port_max,
                protocol: rule.protocol.clone(),
            },
        );
    }

    let mut ip_rules = Vec::with_capacity(config.ip_rules.len());
    for rule in &config.ip_rules {
        let ip_min = rule
            .ip_min
            .parse::<IpAddr>()
            .map_err(|_| PoolError::new(format!("invalid routing ip_min: {}", rule.ip_min)))?;
        let ip_max = rule
            .ip_max
            .as_deref()
            .unwrap_or(&rule.ip_min)
            .parse::<IpAddr>()
            .map_err(|_| {
                PoolError::new(format!(
                    "invalid routing ip_max: {}",
                    rule.ip_max.as_deref().unwrap_or(&rule.ip_min)
                ))
            })?;
        ip_rules.push(smelly_connect::resource::IpRule {
            ip_min,
            ip_max,
            port_min: rule.port_min,
            port_max: rule.port_max,
            protocol: rule.protocol.clone(),
        });
    }

    Ok(LocalRouteOverrides::new(domain_rules, ip_rules))
}

fn normalize_override_domain(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("*.") {
        format!(".{rest}")
    } else {
        trimmed.to_string()
    }
}

fn build_pool_summary(state: &PoolState) -> PoolSummary {
    let mut ready_nodes = 0;
    let mut suspect_nodes = 0;
    let mut open_nodes = 0;
    let mut timed_open_nodes = 0;
    let mut half_open_nodes = 0;
    let mut connecting_nodes = 0;
    let mut configured_nodes = 0;

    for node in &state.nodes {
        match node.state {
            AccountState::Configured(_) => configured_nodes += 1,
            AccountState::Connecting => connecting_nodes += 1,
            AccountState::Ready(_) => ready_nodes += 1,
            AccountState::Suspect(_) => suspect_nodes += 1,
            AccountState::Open(_) => {
                open_nodes += 1;
                if node.open_until.is_some() {
                    timed_open_nodes += 1;
                }
            }
            AccountState::HalfOpen(_) => half_open_nodes += 1,
        }
    }

    let selectable_nodes = ready_nodes + suspect_nodes;
    let status = if selectable_nodes > 0 {
        PoolHealthStatus::Healthy
    } else if half_open_nodes > 0
        || connecting_nodes > 0
        || timed_open_nodes > 0
        || configured_nodes > 0
    {
        PoolHealthStatus::Recovering
    } else {
        PoolHealthStatus::Down
    };

    PoolSummary {
        status,
        total_nodes: state.nodes.len(),
        selectable_nodes,
        ready_nodes,
        suspect_nodes,
        open_nodes,
        half_open_nodes,
        connecting_nodes,
        configured_nodes,
    }
}

fn open_node(node: &mut AccountNode, message: String) {
    node.current_backoff = next_backoff(node.current_backoff, node.backoff_base, node.backoff_max);
    node.open_until = Some(Instant::now() + node.current_backoff);
    node.state = AccountState::Open(AccountFailure { message });
}
