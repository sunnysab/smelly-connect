use std::collections::HashMap;
use std::fmt::{Display, Formatter};
#[cfg(any(test, feature = "test-utils"))]
use std::future::Future;
use std::net::IpAddr;
#[cfg(any(test, feature = "test-utils"))]
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use smelly_connect::domain::route_policy::RoutePolicy;
use smelly_connect::session::normalize_override_domain;
use smelly_connect::{
    CaptchaError, CaptchaHandler, EasyConnectClient, LocalRouteOverrides, Session,
};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::config::{AccountConfig, AppConfig, RoutingDefaultAction};

mod maintenance;
mod selection;
mod snapshot;
mod state;

use maintenance::PoolMaintenance;
use selection::next_selectable_index;
use snapshot::{build_local_route_set_snapshot, build_route_set_snapshot};
pub use snapshot::{
    AccountNodeSnapshot, AccountRoutesSnapshot, PoolHealthStatus, PoolSnapshot, PoolSummary,
    ProbeRaceResult, RoutesSnapshot,
};
use state::disable_node;
#[cfg(any(test, feature = "test-utils"))]
use state::next_backoff;
use state::{build_pool_summary, open_node, state_label};

#[derive(Clone)]
pub struct PooledSession {
    account_name: String,
    session: Option<Session>,
    // Held solely for its Drop side-effect: dropping this stops the ICMP keepalive task.
    _keepalive: Option<Arc<std::sync::Mutex<smelly_connect::KeepaliveHandle>>>,
}

impl PooledSession {
    #[cfg(any(test, feature = "test-utils"))]
    fn new(account_name: String, session: Option<Session>) -> Self {
        Self {
            account_name,
            session,
            _keepalive: None,
        }
    }

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
    pub permanent_auth: bool,
}

impl AccountFailure {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent_auth: false,
        }
    }

    fn permanent_auth(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent_auth: true,
        }
    }
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
    account: AccountConfig,
    state: AccountState,
    reconnect_session: Option<Session>,
    #[allow(dead_code)]
    flaky_retry: bool,
    consecutive_failures: u32,
    failure_threshold: u32,
    current_backoff: Duration,
    backoff_base: Duration,
    backoff_max: Duration,
    open_until: Option<Instant>,
    live_probe_in_flight: bool,
}

#[derive(Default)]
struct PoolState {
    nodes: Vec<AccountNode>,
    cursor: usize,
    total_reconnections: u64,
}

pub struct SessionPool {
    inner: Arc<Mutex<PoolState>>,
    maintenance: Arc<PoolMaintenance>,
    user_refs: Arc<AtomicUsize>,
    counts_for_shutdown: bool,
    healthcheck_interval: Duration,
    #[cfg(any(test, feature = "test-utils"))]
    retry_delay: Duration,
    connect_timeout: Duration,
    local_route_overrides: LocalRouteOverrides,
    route_policy: RoutePolicy,
    allow_all_routes: bool,
    keepalive_target: Option<String>,
    server: Option<String>,
    server_cert_policy: smelly_connect::ServerCertPolicy,
    allow_request_triggered_probe: bool,
    min_pool_size: usize,
}

impl Clone for SessionPool {
    fn clone(&self) -> Self {
        self.clone_with_refcount(true)
    }
}

impl Drop for SessionPool {
    fn drop(&mut self) {
        if self.counts_for_shutdown && self.user_refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.maintenance.abort();
        }
    }
}

const DEFAULT_SESSION_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_VPN_HEALTH_PROBE_ATTEMPTS: usize = 3;
const DEFAULT_VPN_HEALTH_PROBE_DELAY: Duration = Duration::from_millis(200);
const RECOVERY_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Additional margin added to pool.connect_timeout when awaiting a spawned maintenance
/// recovery task. If the inner task does not complete within connect_timeout + margin,
/// the JoinSet is aborted to unblock the maintenance loop. Any node left in Connecting
/// state is reclaimed by refresh_time_based_states on the next cycle.
const MAINTENANCE_TASK_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

#[cfg(any(test, feature = "test-utils"))]
type TestConnectFuture = Pin<Box<dyn Future<Output = Result<Session, PoolError>> + Send>>;

#[cfg(any(test, feature = "test-utils"))]
type TestConnectHook = Arc<dyn Fn(AccountConfig) -> TestConnectFuture + Send + Sync>;

#[cfg(any(test, feature = "test-utils"))]
tokio::task_local! {
    static TEST_CONNECT_HOOK: TestConnectHook;
}

#[cfg(any(test, feature = "test-utils"))]
fn current_test_connect_hook() -> Option<TestConnectHook> {
    TEST_CONNECT_HOOK.try_with(Arc::clone).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    Message(String),
    ClientBuildFailed(smelly_connect::Error),
    SessionConnectFailed(smelly_connect::Error),
    SessionConnectTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolStartupMode {
    RequireReady,
    AllowEmpty,
}

impl PoolError {
    fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    fn client_build_failed(error: smelly_connect::Error) -> Self {
        Self::ClientBuildFailed(error)
    }

    fn session_connect_failed(error: smelly_connect::Error) -> Self {
        Self::SessionConnectFailed(error)
    }

    pub fn underlying_error(&self) -> Option<&smelly_connect::Error> {
        match self {
            Self::ClientBuildFailed(error) | Self::SessionConnectFailed(error) => Some(error),
            Self::Message(_) | Self::SessionConnectTimeout => None,
        }
    }
}

impl Display for PoolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::ClientBuildFailed(error) => {
                write!(f, "pool client build failed: {error:?}")
            }
            Self::SessionConnectFailed(error) => {
                write!(f, "pool session connect failed: {error:?}")
            }
            Self::SessionConnectTimeout => f.write_str("session connect timeout"),
        }
    }
}

impl std::error::Error for PoolError {}

impl SessionPool {
    fn clone_with_refcount(&self, counts_for_shutdown: bool) -> Self {
        if counts_for_shutdown {
            self.user_refs.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            inner: Arc::clone(&self.inner),
            maintenance: Arc::clone(&self.maintenance),
            user_refs: Arc::clone(&self.user_refs),
            counts_for_shutdown,
            healthcheck_interval: self.healthcheck_interval,
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: self.retry_delay,
            connect_timeout: self.connect_timeout,
            local_route_overrides: self.local_route_overrides.clone(),
            route_policy: self.route_policy,
            allow_all_routes: self.allow_all_routes,
            keepalive_target: self.keepalive_target.clone(),
            server: self.server.clone(),
            server_cert_policy: self.server_cert_policy.clone(),
            allow_request_triggered_probe: self.allow_request_triggered_probe,
            min_pool_size: self.min_pool_size,
        }
    }

    pub async fn from_config_allow_empty(cfg: &AppConfig) -> Result<Self, PoolError> {
        Self::from_config_with_startup_mode(cfg, PoolStartupMode::AllowEmpty).await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_config_with_connect_hook_for_test<F>(
        cfg: &AppConfig,
        startup_mode: PoolStartupMode,
        hook: F,
    ) -> Result<Self, PoolError>
    where
        F: Fn(&AccountConfig) -> Result<Session, PoolError> + Send + Sync + 'static,
    {
        Self::from_config_with_async_connect_hook_for_test(cfg, startup_mode, move |account| {
            let result = hook(&account);
            async move { result }
        })
        .await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_config_with_async_connect_hook_for_test<F, Fut>(
        cfg: &AppConfig,
        startup_mode: PoolStartupMode,
        hook: F,
    ) -> Result<Self, PoolError>
    where
        F: Fn(AccountConfig) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Session, PoolError>> + Send + 'static,
    {
        let hook: TestConnectHook = Arc::new(move |account| Box::pin(hook(account)));
        TEST_CONNECT_HOOK
            .scope(hook, Self::from_config_with_startup_mode(cfg, startup_mode))
            .await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_test_accounts(total: usize, ready_count: usize) -> Self {
        let mut nodes = Vec::new();
        for idx in 0..total {
            let name = format!("acct-{:02}", idx + 1);
            let state = if idx < ready_count {
                AccountState::Ready(PooledSession::new(name.clone(), None).into())
            } else {
                AccountState::Configured(AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                })
            };
            nodes.push(AccountNode {
                account: AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                },
                state,
                reconnect_session: None,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
                live_probe_in_flight: false,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_named_ready_accounts<const N: usize>(names: [&str; N]) -> Self {
        let nodes = names
            .into_iter()
            .map(|name| AccountNode {
                account: AccountConfig {
                    name: name.to_string(),
                    username: name.to_string(),
                    password: "pass".to_string(),
                },
                state: AccountState::Ready(PooledSession::new(name.to_string(), None).into()),
                reconnect_session: None,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
                live_probe_in_flight: false,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(feature = "test-utils")]
    pub async fn from_named_ready_live_accounts<const N: usize>(
        entries: [(&str, &str, std::net::Ipv4Addr); N],
    ) -> Self {
        let nodes = entries
            .into_iter()
            .map(|(account_name, host, ip)| AccountNode {
                account: AccountConfig {
                    name: account_name.to_string(),
                    username: account_name.to_string(),
                    password: "pass".to_string(),
                },
                state: AccountState::Ready(
                    PooledSession::new(
                        account_name.to_string(),
                        Some(
                            smelly_connect::test_support::session::session_with_domain_match(
                                host, ip,
                            ),
                        ),
                    )
                    .into(),
                ),
                reconnect_session: None,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
                live_probe_in_flight: false,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_live_sessions_for_test(entries: Vec<(&str, Session)>) -> Self {
        Self::from_live_sessions_with_route_policy_for_test(entries, RoutePolicy::default()).await
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_live_sessions_with_route_policy_for_test(
        entries: Vec<(&str, Session)>,
        route_policy: RoutePolicy,
    ) -> Self {
        let local_route_overrides = LocalRouteOverrides::default();
        let nodes = entries
            .into_iter()
            .map(|(account_name, session)| {
                let session =
                    apply_pool_routing(session, &local_route_overrides, route_policy, false);
                AccountNode {
                    account: AccountConfig {
                        name: account_name.to_string(),
                        username: account_name.to_string(),
                        password: "pass".to_string(),
                    },
                    state: AccountState::Ready(
                        PooledSession::new(account_name.to_string(), Some(session)).into(),
                    ),
                    reconnect_session: None,
                    flaky_retry: false,
                    consecutive_failures: 0,
                    failure_threshold: 3,
                    current_backoff: Duration::from_secs(30),
                    backoff_base: Duration::from_secs(30),
                    backoff_max: Duration::from_secs(600),
                    open_until: None,
                    live_probe_in_flight: false,
                }
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides,
            route_policy,
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_live_sessions_with_keepalive_target_for_test(
        entries: Vec<(&str, Session)>,
        keepalive_target: &str,
    ) -> Self {
        let mut pool = Self::from_live_sessions_for_test(entries).await;
        pool.keepalive_target = Some(keepalive_target.to_string());
        pool
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_live_sessions_with_active_keepalive_for_test(
        entries: Vec<(&str, Session)>,
        keepalive_target: &str,
    ) -> Self {
        let pool =
            Self::from_live_sessions_with_keepalive_target_for_test(entries, keepalive_target)
                .await;
        pool.arm_keepalives_for_live_sessions_for_test().await;
        pool
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_connecting_recovery_for_test(
        account_name: &str,
        session: Session,
        delay: Duration,
    ) -> Self {
        let account = AccountConfig {
            name: account_name.to_string(),
            username: account_name.to_string(),
            password: "pass".to_string(),
        };
        let pool = Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    account: account.clone(),
                    state: AccountState::Connecting,
                    reconnect_session: None,
                    flaky_retry: false,
                    consecutive_failures: 3,
                    failure_threshold: 3,
                    current_backoff: Duration::from_secs(30),
                    backoff_base: Duration::from_secs(30),
                    backoff_max: Duration::from_secs(600),
                    open_until: None,
                    live_probe_in_flight: false,
                }],
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        };
        let inner = Arc::clone(&pool.inner);
        let account_name = account_name.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let mut state = inner.lock().await;
            if let Some(node) = state
                .nodes
                .iter_mut()
                .find(|node| node.account.name == account_name)
            {
                node.state = AccountState::Ready(
                    PooledSession::new(account_name.clone(), Some(session)).into(),
                );
                node.consecutive_failures = 0;
            }
        });
        pool
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_test_outcomes<const N: usize>(
        outcomes: [Result<&str, &str>; N],
        min_ready: usize,
    ) -> Self {
        let mut nodes = Vec::new();
        for (idx, outcome) in outcomes.into_iter().enumerate() {
            let (name, state) = match outcome {
                Ok(name) if idx < min_ready => (
                    name.to_string(),
                    AccountState::Ready(PooledSession::new(name.to_string(), None).into()),
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
                    AccountState::Open(AccountFailure::transient(message)),
                ),
            };
            nodes.push(AccountNode {
                account: AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                },
                state,
                reconnect_session: None,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
                live_probe_in_flight: false,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_failed_accounts(total: usize) -> Self {
        let mut nodes = Vec::new();
        for idx in 0..total {
            let name = format!("failed-{:02}", idx + 1);
            nodes.push(AccountNode {
                account: AccountConfig {
                    name: name.clone(),
                    username: name.clone(),
                    password: "pass".to_string(),
                },
                state: AccountState::Open(AccountFailure::transient("not ready")),
                reconnect_session: None,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: 3,
                current_backoff: Duration::from_secs(30),
                backoff_base: Duration::from_secs(30),
                backoff_max: Duration::from_secs(600),
                open_until: None,
                live_probe_in_flight: false,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_flaky_account_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    account: AccountConfig {
                        name: "acct-01".to_string(),
                        username: "acct-01".to_string(),
                        password: "pass".to_string(),
                    },
                    state: AccountState::Ready(
                        PooledSession::new("acct-01".to_string(), None).into(),
                    ),
                    reconnect_session: None,
                    flaky_retry: true,
                    consecutive_failures: 0,
                    failure_threshold: 3,
                    current_backoff: Duration::from_secs(30),
                    backoff_base: Duration::from_secs(30),
                    backoff_max: Duration::from_secs(600),
                    open_until: None,
                    live_probe_in_flight: false,
                }],
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_millis(100),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    pub async fn from_config(cfg: &AppConfig) -> Result<Self, PoolError> {
        Self::from_config_with_startup_mode(cfg, PoolStartupMode::RequireReady).await
    }

    async fn from_config_with_startup_mode(
        cfg: &AppConfig,
        startup_mode: PoolStartupMode,
    ) -> Result<Self, PoolError> {
        let server_cert_policy = cfg
            .server_cert_policy()
            .map_err(|err| PoolError::new(err.to_string()))?;
        tracing::info!(
            accounts = cfg.accounts.len(),
            min_pool_size = cfg.pool.min_pool_size,
            "pool startup"
        );
        let mut nodes = Vec::new();
        for account in &cfg.accounts {
            nodes.push(AccountNode {
                account: account.clone(),
                state: AccountState::Configured(account.clone()),
                reconnect_session: None,
                flaky_retry: false,
                consecutive_failures: 0,
                failure_threshold: cfg.pool.failure_threshold,
                current_backoff: Duration::from_secs(cfg.pool.backoff_base_secs),
                backoff_base: Duration::from_secs(cfg.pool.backoff_base_secs),
                backoff_max: Duration::from_secs(cfg.pool.backoff_max_secs),
                open_until: None,
                live_probe_in_flight: false,
            });
        }

        let keepalive_target = cfg.icmp_keepalive_target().map(str::to_owned);
        let pool = Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(cfg.pool.healthcheck_interval_secs.max(1)),
            #[cfg(any(test, feature = "test-utils"))]
            retry_delay: Duration::from_secs(cfg.pool.healthcheck_interval_secs.max(1)),
            connect_timeout: cfg.session_connect_timeout(),
            local_route_overrides: build_local_route_overrides(&cfg.routing)?,
            route_policy: route_policy_from_default_action(cfg.routing.default_action),
            allow_all_routes: cfg.routing.allow_all,
            keepalive_target,
            server: Some(cfg.vpn.server.clone()),
            server_cert_policy,
            allow_request_triggered_probe: cfg.pool.allow_request_triggered_probe,
            min_pool_size: cfg.pool.min_pool_size,
        };

        pool.ensure_min_pool_size().await;
        let ready = pool.ready_count().await;
        tracing::info!(
            configured = cfg.accounts.len(),
            min_pool_size = cfg.pool.min_pool_size,
            ready,
            "pool startup summary"
        );
        if ready == 0 {
            match startup_mode {
                PoolStartupMode::RequireReady => {
                    tracing::error!("no ready session after startup");
                    return Err(PoolError::new("no ready session after startup"));
                }
                PoolStartupMode::AllowEmpty => {
                    tracing::warn!("starting with no ready session after startup");
                }
            }
        }
        pool.spawn_background_maintenance_task();
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

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn state_summary_for_test(&self) -> String {
        self.refresh_time_based_states().await;
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .map(|node| {
                let label = state_label(&node.state);
                format!("{}:{label}", node.account.name)
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
                name: node.account.name.clone(),
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

    pub async fn shutdown(&self) {
        self.maintenance.shutdown().await;
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
                name: node.account.name.clone(),
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
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name)
        {
            node.live_probe_in_flight = false;
            if matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_)
            ) {
                node.consecutive_failures = node.failure_threshold;
                open_node(node, AccountFailure::transient(error.clone()));
                tracing::warn!(
                    account = %account_name,
                    reason = %error,
                    failure_threshold = node.failure_threshold,
                    "live session marked open after proxy failure"
                );
            }
        }
    }

    pub async fn report_live_session_unhealthy(
        &self,
        account_name: &str,
        error: impl Into<String>,
    ) {
        let error = error.into();
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name)
            && matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_)
            )
        {
            node.consecutive_failures = node.failure_threshold;
            open_node(node, AccountFailure::transient(error.clone()));
            tracing::warn!(
                account = %account_name,
                reason = %error,
                failure_threshold = node.failure_threshold,
                backoff_secs = node.current_backoff.as_secs(),
                "live session marked unhealthy after vpn probe failures"
            );
        }
    }

    pub async fn report_live_session_reconnect_required(
        &self,
        account_name: &str,
        session: &Session,
        error: impl Into<String>,
    ) {
        let error = error.into();
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name)
            && matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_)
            )
        {
            node.reconnect_session = Some(session.clone());
            node.consecutive_failures = node.failure_threshold;
            open_node(node, AccountFailure::transient(error.clone()));
            tracing::warn!(
                account = %account_name,
                reason = %error,
                failure_threshold = node.failure_threshold,
                backoff_secs = node.current_backoff.as_secs(),
                "live session retired and queued for reconnect"
            );
        }
    }

    pub async fn report_live_session_unhealthy_if_probe_fails(
        &self,
        account_name: &str,
        session: &Session,
        error: impl Into<String>,
    ) {
        let Some(target) = self.keepalive_target.clone() else {
            return;
        };
        if !self.claim_live_session_probe(account_name).await {
            return;
        }
        let account_name = account_name.to_string();
        let error = error.into();
        let pool = self.clone_with_refcount(false);
        let session = session.clone();
        tokio::spawn(async move {
            let result = probe_live_session_health(
                &session,
                smelly_connect::session::IcmpKeepAliveTarget::from(target),
            )
            .await;
            if result.is_ok() {
                pool.clear_live_session_probe(&account_name).await;
            } else {
                pool.report_live_session_unhealthy(&account_name, error)
                    .await;
            }
        });
    }

    fn spawn_background_maintenance_task(&self) {
        let interval = self.healthcheck_interval;
        let pool = self.clone_with_refcount(false);
        let maintenance = Arc::clone(&self.maintenance);
        let mut shutdown = self.maintenance.subscribe();
        self.maintenance.install(tokio::spawn(async move {
            maintenance.running.store(true, Ordering::Release);
            if *shutdown.borrow() {
                maintenance.running.store(false, Ordering::Release);
                return;
            }
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(interval) => {
                        pool.run_periodic_maintenance_once().await;
                    }
                }
            }
            maintenance.running.store(false, Ordering::Release);
        }));
    }

    async fn run_periodic_maintenance_once(&self) {
        self.ensure_min_pool_size().await;
        if self.keepalive_target.is_some() {
            self.run_periodic_healthcheck_once().await;
        }
    }

    async fn collect_periodic_probe_targets(&self) -> Vec<(String, Session)> {
        if self.keepalive_target.is_none() {
            return Vec::new();
        }

        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        let mut sessions = Vec::new();
        for node in &mut state.nodes {
            if node.live_probe_in_flight {
                continue;
            }
            let Some(session) = (match &node.state {
                AccountState::Ready(session) | AccountState::Suspect(session) => {
                    session.session().cloned()
                }
                _ => None,
            }) else {
                continue;
            };
            node.live_probe_in_flight = true;
            sessions.push((node.account.name.clone(), session));
        }
        sessions
    }

    async fn run_periodic_healthcheck_once(&self) {
        let Some(target) = self.keepalive_target.clone() else {
            return;
        };

        for (account_name, session) in self.collect_periodic_probe_targets().await {
            let result = probe_live_session_health(
                &session,
                smelly_connect::session::IcmpKeepAliveTarget::from(target.clone()),
            )
            .await;
            if result.is_ok() {
                self.clear_live_session_probe(&account_name).await;
            } else {
                self.report_live_session_unhealthy(&account_name, "background healthcheck failed")
                    .await;
            }
        }
    }

    fn build_keepalive_handle(
        &self,
        account_name: &str,
        session: &Session,
    ) -> Option<Arc<std::sync::Mutex<smelly_connect::KeepaliveHandle>>> {
        let target = self.keepalive_target.clone()?;
        let pool = self.clone_with_refcount(false);
        let account_name = account_name.to_string();
        Some(Arc::new(std::sync::Mutex::new(
            session.start_icmp_keepalive_with_failure_handler(
                target,
                DEFAULT_SESSION_KEEPALIVE_INTERVAL,
                move || {
                    let pool = pool.clone_with_refcount(false);
                    let account_name = account_name.clone();
                    tokio::spawn(async move {
                        pool.report_live_session_unhealthy(
                            &account_name,
                            "session keepalive failed",
                        )
                        .await;
                    });
                },
            ),
        )))
    }

    fn wrap_live_session(&self, account_name: String, session: Session) -> PooledSession {
        let keepalive = self.build_keepalive_handle(&account_name, &session);
        PooledSession {
            account_name,
            session: Some(session),
            _keepalive: keepalive,
        }
    }

    async fn claim_live_session_probe(&self, account_name: &str) -> bool {
        let mut state = self.inner.lock().await;
        let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name)
        else {
            return false;
        };
        if !matches!(
            node.state,
            AccountState::Ready(_) | AccountState::Suspect(_)
        ) {
            return false;
        }
        if node.live_probe_in_flight {
            return false;
        }
        node.live_probe_in_flight = true;
        true
    }

    async fn clear_live_session_probe(&self, account_name: &str) {
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name)
        {
            node.live_probe_in_flight = false;
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
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

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_mixed_state_pool_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![
                    AccountNode {
                        account: AccountConfig {
                            name: "ready-01".to_string(),
                            username: "ready-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::Ready(
                            PooledSession::new("ready-01".to_string(), None).into(),
                        ),
                        reconnect_session: None,
                        flaky_retry: false,
                        consecutive_failures: 0,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: None,
                        live_probe_in_flight: false,
                    },
                    AccountNode {
                        account: AccountConfig {
                            name: "suspect-01".to_string(),
                            username: "suspect-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::Suspect(
                            PooledSession::new("suspect-01".to_string(), None).into(),
                        ),
                        reconnect_session: None,
                        flaky_retry: false,
                        consecutive_failures: 1,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: None,
                        live_probe_in_flight: false,
                    },
                    AccountNode {
                        account: AccountConfig {
                            name: "open-01".to_string(),
                            username: "open-01".to_string(),
                            password: "pass".to_string(),
                        },
                        state: AccountState::Open(AccountFailure::transient("open")),
                        reconnect_session: None,
                        flaky_retry: false,
                        consecutive_failures: 3,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: Some(Instant::now() + Duration::from_secs(30)),
                        live_probe_in_flight: false,
                    },
                    AccountNode {
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
                        reconnect_session: None,
                        flaky_retry: false,
                        consecutive_failures: 3,
                        failure_threshold: 3,
                        current_backoff: Duration::from_secs(30),
                        backoff_base: Duration::from_secs(30),
                        backoff_max: Duration::from_secs(600),
                        open_until: None,
                        live_probe_in_flight: false,
                    },
                ],
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn from_exhausted_pool_for_test() -> Self {
        let account = AccountConfig {
            name: "acct-01".to_string(),
            username: "acct-01".to_string(),
            password: "pass".to_string(),
        };
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    account: account.clone(),
                    state: AccountState::Open(AccountFailure::transient("vpn unavailable")),
                    reconnect_session: None,
                    flaky_retry: false,
                    consecutive_failures: 3,
                    failure_threshold: 3,
                    current_backoff: Duration::from_secs(30),
                    backoff_base: Duration::from_secs(30),
                    backoff_max: Duration::from_secs(600),
                    open_until: Some(Instant::now() + Duration::from_secs(30)),
                    live_probe_in_flight: false,
                }],
                cursor: 0,
                total_reconnections: 0,
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            healthcheck_interval: Duration::from_secs(60),
            retry_delay: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(20),
            local_route_overrides: LocalRouteOverrides::default(),
            route_policy: RoutePolicy::default(),
            allow_all_routes: false,
            keepalive_target: None,
            server: None,
            server_cert_policy: smelly_connect::ServerCertPolicy::Verify,
            allow_request_triggered_probe: true,
            min_pool_size: 0,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
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

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn current_backoff_for_test(&self) -> Duration {
        let state = self.inner.lock().await;
        state
            .nodes
            .first()
            .map(|node| node.current_backoff)
            .unwrap_or_default()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn connect_timeout_for_test(&self) -> Duration {
        self.connect_timeout
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn report_auth_failure_for_test(&self, account_name: &str, error: PoolError) {
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name)
        {
            disable_node(node, account_failure_from_pool_error(&error));
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn try_request_triggered_probe_for_test(&self) -> Result<PooledSession, PoolError> {
        let Some((name, account, _reconnect_session)) =
            self.claim_request_triggered_probe().await?
        else {
            return Err(PoolError::new("no ready session"));
        };
        let session = PooledSession::new(name.clone(), None);
        self.complete_probe_success(&name, session.clone(), account)
            .await?;
        Ok(session)
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn run_concurrent_probe_race_for_test(&self) -> ProbeRaceResult {
        let first = {
            let pool = self.clone_with_refcount(false);
            tokio::spawn(async move { pool.try_request_triggered_probe_for_test().await })
        };
        let second = {
            let pool = self.clone_with_refcount(false);
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

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn force_probe_failure_for_test(&self) {
        let mut state = self.inner.lock().await;
        if let Some(node) = state.nodes.first_mut() {
            node.current_backoff =
                next_backoff(node.current_backoff, node.backoff_base, node.backoff_max);
            node.open_until = Some(Instant::now() + node.current_backoff);
            node.state = AccountState::Open(AccountFailure::transient("forced probe failure"));
            let name = node.account.name.clone();
            let backoff = node.current_backoff;
            let account = node.account.clone();
            let inner = Arc::clone(&self.inner);
            drop(state);
            tokio::spawn(async move {
                tokio::time::sleep(backoff).await;
                let mut state = inner.lock().await;
                if let Some(node) = state
                    .nodes
                    .iter_mut()
                    .find(|node| node.account.name == name)
                {
                    node.state = AccountState::HalfOpen(account);
                }
            });
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn next_account_name(&self) -> Result<String, PoolError> {
        Ok(self.next_session().await?.account_name().to_string())
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn run_periodic_healthcheck_once_for_test(&self) {
        self.run_periodic_healthcheck_once().await;
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn start_background_maintenance_for_test(&self) {
        self.spawn_background_maintenance_task();
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn background_maintenance_running_flag_for_test(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.maintenance.running)
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn keepalive_target_for_test(&self) -> Option<String> {
        self.keepalive_target.clone()
    }

    #[cfg(any(test, feature = "test-utils"))]
    async fn arm_keepalives_for_live_sessions_for_test(&self) {
        let mut state = self.inner.lock().await;
        for node in &mut state.nodes {
            match &mut node.state {
                AccountState::Ready(session) | AccountState::Suspect(session) => {
                    if let Some(live) = session.session.as_ref() {
                        session._keepalive =
                            self.build_keepalive_handle(session.account_name(), live);
                    }
                }
                _ => {}
            }
        }
    }

    pub async fn next_session(&self) -> Result<PooledSession, PoolError> {
        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        let Some(idx) = next_selectable_index(&mut state, |node| {
            matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_)
            )
        }) else {
            return Err(PoolError::new("no ready session"));
        };

        match &state.nodes[idx].state {
            AccountState::Ready(session) | AccountState::Suspect(session) => {
                Ok(session.as_ref().clone())
            }
            _ => Err(PoolError::new("no ready session")),
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn ensure_additional_capacity_for_test(&self) -> Result<(), PoolError> {
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| matches!(node.state, AccountState::Configured(_)))
        {
            node.state =
                AccountState::Ready(PooledSession::new(node.account.name.clone(), None).into());
            return Ok(());
        }
        Err(PoolError::new("no configurable account remaining"))
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn force_one_failure_for_test(&self) {
        self.force_failures_for_test(1).await;
    }

    #[cfg(any(test, feature = "test-utils"))]
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
                    let name = node.account.name.clone();
                    let flaky_retry = node.flaky_retry;
                    node.live_probe_in_flight = false;
                    node.consecutive_failures += 1;

                    let session = match std::mem::replace(
                        &mut node.state,
                        AccountState::Open(AccountFailure::transient("forced failure")),
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
                    if let Some(node) = state
                        .nodes
                        .iter_mut()
                        .find(|node| node.account.name == name)
                    {
                        node.state = AccountState::Ready(
                            PooledSession::new(node.account.name.clone(), None).into(),
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

        if self.has_connecting_nodes().await {
            let deadline = Instant::now() + self.connect_timeout;
            while Instant::now() < deadline {
                tokio::time::sleep(RECOVERY_WAIT_POLL_INTERVAL).await;
                if let Some(ready) = self.next_ready_with_session().await? {
                    return Ok(ready);
                }
                if !self.has_connecting_nodes().await {
                    break;
                }
            }
        }

        Err(PoolError::new("no ready session"))
    }

    async fn ensure_min_pool_size(&self) {
        let target = self.min_pool_size;
        if target == 0 {
            return;
        }

        #[cfg(any(test, feature = "test-utils"))]
        let test_connect_hook = current_test_connect_hook();
        let mut pending = JoinSet::new();

        loop {
            self.refresh_time_based_states().await;
            let ready = self.ready_count().await;
            if ready >= target {
                break;
            }

            while ready + pending.len() < target {
                // Prefer connecting fresh Configured accounts, fall back to HalfOpen.
                if self.has_configured_accounts().await {
                    let pool = self.clone_with_refcount(false);
                    #[cfg(any(test, feature = "test-utils"))]
                    let test_connect_hook = test_connect_hook.clone();
                    pending.spawn(async move {
                        pool.connect_one_configured_with_test_hook(
                            #[cfg(any(test, feature = "test-utils"))]
                            test_connect_hook,
                        )
                        .await
                    });
                } else if let Some((name, account, reconnect_session)) =
                    self.claim_maintenance_probe().await
                {
                    let pool = self.clone_with_refcount(false);
                    pending.spawn(async move {
                        pool.recover_and_complete_probe(&name, &account, reconnect_session)
                            .await
                    });
                } else {
                    break;
                }
            }

            let task_timeout = self.connect_timeout + MAINTENANCE_TASK_TIMEOUT_MARGIN;
            match tokio::time::timeout(task_timeout, pending.join_next()).await {
                Ok(Some(Ok(Ok(()))) | Some(Ok(Err(_)))) => {}
                Ok(Some(Err(err))) if err.is_panic() => {
                    std::panic::resume_unwind(err.into_panic());
                }
                Ok(Some(Err(err))) => {
                    tracing::warn!(
                        error = %err,
                        "pool maintenance task did not complete cleanly"
                    );
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    tracing::error!(
                        timeout_secs = task_timeout.as_secs(),
                        "pool maintenance task hung; aborting pending tasks"
                    );
                    pending.abort_all();
                    break;
                }
            }
        }
    }

    async fn next_ready_with_session(&self) -> Result<Option<(String, Session)>, PoolError> {
        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        let selectable_indices: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| match &node.state {
                AccountState::Ready(session) | AccountState::Suspect(session)
                    if session.session().is_some() =>
                {
                    Some(idx)
                }
                _ => None,
            })
            .collect();
        if selectable_indices.is_empty() {
            return Ok(None);
        }
        let pos = state.cursor % selectable_indices.len();
        state.cursor += 1;
        let idx = selectable_indices[pos];

        match &state.nodes[idx].state {
            AccountState::Ready(session) | AccountState::Suspect(session) => {
                let account_name = session.account_name().to_string();
                let live = session.session().cloned();
                Ok(live.map(|live| (account_name, live)))
            }
            _ => Ok(None),
        }
    }

    async fn has_connecting_nodes(&self) -> bool {
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .any(|node| matches!(node.state, AccountState::Connecting))
    }

    async fn has_configured_accounts(&self) -> bool {
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .any(|node| matches!(node.state, AccountState::Configured(_)))
    }

    async fn connect_one_configured(&self) -> Result<(), PoolError> {
        self.connect_one_configured_with_test_hook(
            #[cfg(any(test, feature = "test-utils"))]
            current_test_connect_hook(),
        )
        .await
    }

    async fn connect_one_configured_with_test_hook(
        &self,
        #[cfg(any(test, feature = "test-utils"))] test_connect_hook: Option<TestConnectHook>,
    ) -> Result<(), PoolError> {
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
            let name = state.nodes[idx].account.name.clone();
            state.nodes[idx].state = AccountState::Connecting;
            state.nodes[idx].open_until = Some(Instant::now() + self.connect_timeout);
            (name, account, server)
        };

        match connect_account(
            &server,
            &account,
            self.connect_timeout,
            ConnectAccountContext {
                local_route_overrides: &self.local_route_overrides,
                route_policy: self.route_policy,
                allow_all_routes: self.allow_all_routes,
                _keepalive_target: self.keepalive_target.as_deref(),
                server_cert_policy: self.server_cert_policy.clone(),
                #[cfg(any(test, feature = "test-utils"))]
                test_connect_hook,
            },
        )
        .await
        {
            Ok(session) => {
                let pooled = self.wrap_live_session(account.name.clone(), session);
                let mut state = self.inner.lock().await;
                if let Some(node) = state
                    .nodes
                    .iter_mut()
                    .find(|node| node.account.name == name)
                {
                    node.state = AccountState::Ready(pooled.into());
                    tracing::info!(account = %account.name, "account ready");
                }
                Ok(())
            }
            Err(err) => {
                let mut state = self.inner.lock().await;
                if let Some(node) = state
                    .nodes
                    .iter_mut()
                    .find(|node| node.account.name == name)
                {
                    let failure = account_failure_from_pool_error(&err);
                    if failure.permanent_auth {
                        disable_node(node, failure);
                    } else {
                        open_node(node, failure);
                    }
                }
                tracing::warn!(account = %account.name, error = %err, "account connect failed");
                Err(err)
            }
        }
    }

    async fn claim_request_triggered_probe(
        &self,
    ) -> Result<Option<(String, AccountConfig, Option<Session>)>, PoolError> {
        if !self.allow_request_triggered_probe {
            return Ok(None);
        }

        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;
        if state.nodes.iter().any(|node| {
            matches!(
                node.state,
                AccountState::Ready(_) | AccountState::Suspect(_)
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
        let name = node.account.name.clone();
        let reconnect_session = node.reconnect_session.clone();
        node.state = AccountState::Connecting;
        node.open_until = Some(Instant::now() + self.connect_timeout);
        tracing::info!(account = %name, "request-triggered recovery probe scheduled");
        Ok(Some((name, account, reconnect_session)))
    }

    async fn try_request_triggered_live_probe(
        &self,
    ) -> Result<Option<(String, Session)>, PoolError> {
        let Some((name, account, reconnect_session)) = self.claim_request_triggered_probe().await?
        else {
            return Ok(None);
        };
        match self
            .recover_account_session(&name, &account, reconnect_session)
            .await
        {
            Ok(session) => {
                let live = session.clone();
                let pooled = self.wrap_live_session(name.clone(), session);
                self.complete_probe_success(&name, pooled, account).await?;
                Ok(Some((name, live)))
            }
            Err(err) => {
                self.complete_probe_failure(&name, account_failure_from_pool_error(&err))
                    .await?;
                Err(err)
            }
        }
    }

    async fn claim_maintenance_probe(&self) -> Option<(String, AccountConfig, Option<Session>)> {
        self.refresh_time_based_states().await;
        let mut state = self.inner.lock().await;

        let probe_candidates: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| {
                (!node.live_probe_in_flight && matches!(node.state, AccountState::HalfOpen(_)))
                    .then_some(idx)
            })
            .collect();
        if probe_candidates.is_empty() {
            return None;
        }

        let pos = state.cursor % probe_candidates.len();
        state.cursor += 1;
        let idx = probe_candidates[pos];
        let node = &mut state.nodes[idx];
        let account = node.account.clone();
        let name = node.account.name.clone();
        let reconnect_session = node.reconnect_session.clone();
        node.state = AccountState::Connecting;
        node.open_until = Some(Instant::now() + self.connect_timeout);
        node.live_probe_in_flight = true;
        tracing::info!(account = %name, "maintenance recovery probe scheduled");
        Some((name, account, reconnect_session))
    }

    async fn recover_and_complete_probe(
        &self,
        name: &str,
        account: &AccountConfig,
        reconnect_session: Option<Session>,
    ) -> Result<(), PoolError> {
        match self
            .recover_account_session(name, account, reconnect_session)
            .await
        {
            Ok(session) => {
                let pooled = self.wrap_live_session(name.to_string(), session);
                let mut state = self.inner.lock().await;
                if let Some(node) = state
                    .nodes
                    .iter_mut()
                    .find(|node| node.account.name == name)
                {
                    node.account = account.clone();
                    node.consecutive_failures = 0;
                    node.current_backoff = node.backoff_base;
                    node.open_until = None;
                    node.reconnect_session = None;
                    node.live_probe_in_flight = false;
                    node.state = AccountState::Ready(Box::new(pooled));
                    state.total_reconnections += 1;
                    tracing::info!(
                        account = %name,
                        reconnects = state.total_reconnections,
                        "maintenance recovery probe succeeded"
                    );
                }
                Ok(())
            }
            Err(err) => {
                let mut state = self.inner.lock().await;
                if let Some(node) = state
                    .nodes
                    .iter_mut()
                    .find(|node| node.account.name == name)
                {
                    node.live_probe_in_flight = false;
                    let failure = account_failure_from_pool_error(&err);
                    if failure.permanent_auth {
                        disable_node(node, failure);
                    } else {
                        open_node(node, failure);
                    }
                }
                tracing::warn!(account = %name, error = %err, "maintenance recovery probe failed");
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
            .find(|node| node.account.name == name)
            .ok_or_else(|| PoolError::new(format!("probe target disappeared: {name}")))?;
        node.account = account;
        node.consecutive_failures = 0;
        node.current_backoff = node.backoff_base;
        node.open_until = None;
        node.reconnect_session = None;
        node.state = AccountState::Ready(Box::new(session));
        state.total_reconnections += 1;
        tracing::info!(
            account = %name,
            reconnects = state.total_reconnections,
            "request-triggered recovery probe succeeded"
        );
        Ok(())
    }

    async fn complete_probe_failure(
        &self,
        name: &str,
        failure: AccountFailure,
    ) -> Result<(), PoolError> {
        let mut state = self.inner.lock().await;
        let node = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == name)
            .ok_or_else(|| PoolError::new(format!("probe target disappeared: {name}")))?;
        if failure.permanent_auth {
            disable_node(node, failure.clone());
        } else {
            open_node(node, failure.clone());
        }
        tracing::warn!(account = %name, error = %failure.message, "request-triggered recovery probe failed");
        Ok(())
    }

    async fn refresh_time_based_states(&self) {
        let mut state = self.inner.lock().await;
        let now = Instant::now();
        for node in &mut state.nodes {
            match &node.state {
                AccountState::Open(_) if node.open_until.is_some_and(|t| now >= t) => {
                    node.state = AccountState::HalfOpen(node.account.clone());
                    node.open_until = None;
                }
                AccountState::Connecting if node.open_until.is_some_and(|t| now >= t) => {
                    tracing::warn!(
                        account = %node.account.name,
                        "connecting timed out, degrading to open"
                    );
                    node.live_probe_in_flight = false;
                    open_node(node, AccountFailure::transient("connecting timed out"));
                }
                _ => {}
            }
        }
    }
}

fn account_failure_from_pool_error(error: &PoolError) -> AccountFailure {
    if is_permanent_auth_failure(error) {
        AccountFailure::permanent_auth(error.to_string())
    } else {
        AccountFailure::transient(error.to_string())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub fn is_permanent_auth_failure_for_test(error: &PoolError) -> bool {
    is_permanent_auth_failure(error)
}

fn is_permanent_auth_failure(error: &PoolError) -> bool {
    error
        .underlying_error()
        .is_some_and(smelly_connect::Error::is_permanent_auth_failure)
}

impl SessionPool {
    async fn recover_account_session(
        &self,
        name: &str,
        account: &AccountConfig,
        reconnect_session: Option<Session>,
    ) -> Result<Session, PoolError> {
        if let Some(session) = reconnect_session {
            match session.rebuild_transport_from_existing_lease().await {
                Ok(rebuilt) => {
                    tracing::info!(
                        account = %name,
                        "live session transport rebuilt"
                    );
                    return Ok(rebuilt);
                }
                Err(err) => {
                    tracing::warn!(
                        account = %name,
                        error = ?err,
                        "live session transport rebuild failed; falling back to full reconnect"
                    );
                }
            }
        }

        let server = self
            .server
            .as_deref()
            .ok_or_else(|| PoolError::new("real server configuration unavailable"))?;

        connect_account(
            server,
            account,
            self.connect_timeout,
            ConnectAccountContext {
                local_route_overrides: &self.local_route_overrides,
                route_policy: self.route_policy,
                allow_all_routes: self.allow_all_routes,
                _keepalive_target: self.keepalive_target.as_deref(),
                server_cert_policy: self.server_cert_policy.clone(),
                #[cfg(any(test, feature = "test-utils"))]
                test_connect_hook: current_test_connect_hook(),
            },
        )
        .await
    }
}

async fn probe_live_session_health(
    session: &Session,
    target: smelly_connect::session::IcmpKeepAliveTarget,
) -> Result<(), ()> {
    let resolved_target = match target {
        smelly_connect::session::IcmpKeepAliveTarget::Ip(ip) => ip,
        host @ smelly_connect::session::IcmpKeepAliveTarget::Host(_) => {
            session.resolve_icmp_target(host).await.map_err(|_| ())?
        }
    };

    for attempt in 0..DEFAULT_VPN_HEALTH_PROBE_ATTEMPTS {
        if session.icmp_ping_ip(resolved_target).await.is_ok() {
            return Ok(());
        }
        if attempt + 1 < DEFAULT_VPN_HEALTH_PROBE_ATTEMPTS {
            tokio::time::sleep(DEFAULT_VPN_HEALTH_PROBE_DELAY).await;
        }
    }

    Err(())
}

struct ConnectAccountContext<'a> {
    local_route_overrides: &'a LocalRouteOverrides,
    route_policy: RoutePolicy,
    allow_all_routes: bool,
    _keepalive_target: Option<&'a str>,
    server_cert_policy: smelly_connect::ServerCertPolicy,
    #[cfg(any(test, feature = "test-utils"))]
    test_connect_hook: Option<TestConnectHook>,
}

async fn connect_account(
    server: &str,
    account: &AccountConfig,
    timeout: Duration,
    ctx: ConnectAccountContext<'_>,
) -> Result<Session, PoolError> {
    #[cfg(any(test, feature = "test-utils"))]
    if let Some(hook) = ctx.test_connect_hook {
        return hook(account.clone()).await.map(|session| {
            apply_pool_routing(
                session,
                ctx.local_route_overrides,
                ctx.route_policy,
                ctx.allow_all_routes,
            )
        });
    }

    let client = EasyConnectClient::builder(server.to_string())
        .credentials(account.username.clone(), account.password.clone())
        .with_server_cert_policy(ctx.server_cert_policy)
        .with_captcha_handler(CaptchaHandler::from_async(|_, _| async move {
            Err(CaptchaError::new(
                "captcha callback not configured for smelly-connect-cli",
            ))
        }))
        .build()
        .map_err(PoolError::client_build_failed)?;

    let session = tokio::time::timeout(timeout, client.connect())
        .await
        .map_err(|_| PoolError::SessionConnectTimeout)?
        .map_err(PoolError::session_connect_failed)?;
    let session = apply_pool_routing(
        session,
        ctx.local_route_overrides,
        ctx.route_policy,
        ctx.allow_all_routes,
    );
    Ok(session)
}

fn apply_pool_routing(
    session: Session,
    local_route_overrides: &LocalRouteOverrides,
    route_policy: RoutePolicy,
    allow_all_routes: bool,
) -> Session {
    let merged_local_route_overrides =
        merge_local_route_overrides(session.local_route_overrides(), local_route_overrides);
    session
        .with_local_route_overrides(merged_local_route_overrides)
        .with_route_policy(route_policy)
        .with_allow_all_routes(allow_all_routes)
}

fn merge_local_route_overrides(
    session_local_route_overrides: &LocalRouteOverrides,
    pool_local_route_overrides: &LocalRouteOverrides,
) -> LocalRouteOverrides {
    let mut domain_rules = session_local_route_overrides.domain_rules().clone();
    domain_rules.extend(pool_local_route_overrides.domain_rules().clone());

    let mut ip_rules = session_local_route_overrides.ip_rules().to_vec();
    ip_rules.extend_from_slice(pool_local_route_overrides.ip_rules());

    LocalRouteOverrides::new(domain_rules, ip_rules)
}

fn route_policy_from_default_action(default_action: RoutingDefaultAction) -> RoutePolicy {
    match default_action {
        RoutingDefaultAction::Direct => RoutePolicy::direct_non_resource_targets(),
        RoutingDefaultAction::Block => RoutePolicy::block_non_resource_targets(),
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
                protocol: rule.protocol,
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
            protocol: rule.protocol,
        });
    }

    Ok(LocalRouteOverrides::new(domain_rules, ip_rules))
}

#[cfg(test)]
mod tests {
    use super::PoolError;

    #[test]
    fn pool_error_preserves_typed_smelly_connect_error() {
        let source = smelly_connect::Error::Transport(
            smelly_connect::error::TransportError::ConnectTimedOut,
        );
        let err = PoolError::session_connect_failed(source.clone());

        assert!(matches!(err, PoolError::SessionConnectFailed(_)));
        assert_eq!(err.underlying_error(), Some(&source));
    }
}
