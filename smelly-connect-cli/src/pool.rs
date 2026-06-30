use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use smelly_connect::domain::route_policy::RoutePolicy;
use smelly_connect::session::normalize_override_domain;
use smelly_connect::{
    CaptchaError, CaptchaHandler, EasyConnectClient, LocalRouteOverrides, Session,
};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::{AccountConfig, AppConfig, RoutingDefaultAction};

mod maintenance;
mod selection;
mod snapshot;
mod state;

use maintenance::PoolMaintenance;
use selection::next_selectable_index;
pub use snapshot::{
    AccountNodeSnapshot, AccountRoutesSnapshot, PoolHealthStatus, PoolSnapshot, PoolSummary,
    RoutesSnapshot,
};
use snapshot::{build_local_route_set_snapshot, build_route_set_snapshot};
use state::{build_pool_summary, state_label};

#[derive(Clone)]
pub struct PooledSession {
    account_name: String,
    session: Option<Session>,
    // Held solely for its Drop side-effect: dropping this stops the ICMP keepalive task.
    _keepalive: Option<Arc<std::sync::Mutex<smelly_connect::KeepaliveHandle>>>,
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

#[derive(Clone)]
pub enum AccountState {
    Idle,
    Connecting,
    Active(Box<PooledSession>),
    Dead,
    Disabled,
}

#[derive(Clone)]
struct AccountNode {
    account: AccountConfig,
    state: AccountState,
    backoff: Duration,
    backoff_until: Option<Instant>,
    probe_in_flight: bool,
}

struct PoolState {
    nodes: Vec<AccountNode>,
    cursor: usize,
    total_reconnections: u64,
    notify: Arc<tokio::sync::Notify>,
}

#[derive(Clone)]
pub(crate) struct PoolConfig {
    healthcheck_interval: Duration,
    connect_timeout: Duration,
    local_route_overrides: LocalRouteOverrides,
    route_policy: RoutePolicy,
    allow_all_routes: bool,
    keepalive_target: Option<String>,
    server: Option<String>,
    server_cert_policy: smelly_connect::ServerCertPolicy,
    min_pool_size: usize,
    backoff_base: Duration,
    backoff_max: Duration,
}

pub struct SessionPool {
    inner: Arc<Mutex<PoolState>>,
    maintenance: Arc<PoolMaintenance>,
    user_refs: Arc<AtomicUsize>,
    counts_for_shutdown: bool,
    config: PoolConfig,
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
const ACQUIRE_NOTIFY_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    Message(String),
    ClientBuildFailed(smelly_connect::Error),
    SessionConnectFailed(smelly_connect::Error),
    /// No node is available to serve the request (all are Dead/Connecting/Disabled/Idle).
    NoReadyNode,
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
            Self::Message(_) | Self::NoReadyNode => None,
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
            Self::NoReadyNode => f.write_str("no ready node available"),
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
            config: self.config.clone(),
        }
    }

    pub async fn from_config_allow_empty(cfg: &AppConfig) -> Result<Self, PoolError> {
        Self::from_config_with_startup_mode(cfg, PoolStartupMode::AllowEmpty).await
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
        let nodes: Vec<AccountNode> = cfg
            .accounts
            .iter()
            .map(|account| AccountNode {
                account: account.clone(),
                state: AccountState::Idle,
                backoff: Duration::from_secs(cfg.pool.backoff_base_secs),
                backoff_until: None,
                probe_in_flight: false,
            })
            .collect();

        let keepalive_target = cfg.icmp_keepalive_target().map(str::to_owned);
        let config = PoolConfig {
            healthcheck_interval: Duration::from_secs(cfg.pool.healthcheck_interval_secs.max(1)),
            connect_timeout: cfg.session_connect_timeout(),
            local_route_overrides: build_local_route_overrides(&cfg.routing)?,
            route_policy: route_policy_from_default_action(cfg.routing.default_action),
            allow_all_routes: cfg.routing.allow_all,
            keepalive_target,
            server: Some(cfg.vpn.server.clone()),
            server_cert_policy,
            min_pool_size: cfg.pool.min_pool_size,
            backoff_base: Duration::from_secs(cfg.pool.backoff_base_secs),
            backoff_max: Duration::from_secs(cfg.pool.backoff_max_secs),
        };
        let pool = Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes,
                cursor: 0,
                total_reconnections: 0,
                notify: Arc::new(tokio::sync::Notify::new()),
            })),
            maintenance: PoolMaintenance::new_shared(),
            user_refs: Arc::new(AtomicUsize::new(1)),
            counts_for_shutdown: true,
            config,
        };

        // Retry connecting until we hit min_pool_size or exhaust Idle nodes.
        // Each maintenance_tick spawns connect tasks for the deficit, but a
        // failed connect marks the node Dead – without a retry loop the pool
        // would give up after the first batch of failures.
        loop {
            let join_handles = pool.maintenance_tick().await;
            if join_handles.is_empty() {
                break;
            }
            for h in join_handles {
                let _ = tokio::time::timeout(Duration::from_secs(30), h).await;
            }
            let state = pool.inner.lock().await;
            if pool.active_count_locked(&state) >= cfg.pool.min_pool_size {
                break;
            }
        }
        let active = {
            let state = pool.inner.lock().await;
            pool.active_count_locked(&state)
        };
        tracing::info!(
            accounts = cfg.accounts.len(),
            min_pool_size = cfg.pool.min_pool_size,
            active,
            "pool startup summary"
        );
        if active == 0 {
            match startup_mode {
                PoolStartupMode::RequireReady => {
                    tracing::error!("no active session after startup");
                    return Err(PoolError::new("no active session after startup"));
                }
                PoolStartupMode::AllowEmpty => {
                    tracing::warn!("starting with no active session after startup");
                }
            }
        }
        pool.spawn_background_maintenance_task();
        Ok(pool)
    }

    fn active_count_locked(&self, state: &PoolState) -> usize {
        state
            .nodes
            .iter()
            .filter(|node| matches!(node.state, AccountState::Active(_)))
            .count()
    }

    pub async fn ready_count(&self) -> usize {
        let state = self.inner.lock().await;
        self.active_count_locked(&state)
    }

    pub async fn snapshot(&self) -> PoolSnapshot {
        let state = self.inner.lock().await;
        let summary = build_pool_summary(&state);
        let nodes = state
            .nodes
            .iter()
            .map(|node| AccountNodeSnapshot {
                name: node.account.name.clone(),
                state: state_label(&node.state).to_ascii_lowercase(),
            })
            .collect();

        PoolSnapshot { summary, nodes }
    }

    pub async fn summary(&self) -> PoolSummary {
        let state = self.inner.lock().await;
        build_pool_summary(&state)
    }

    pub async fn shutdown(&self) {
        self.maintenance.shutdown().await;
    }

    pub async fn routes_snapshot(&self) -> RoutesSnapshot {
        let state = self.inner.lock().await;
        let mut nodes = Vec::with_capacity(state.nodes.len());

        for node in &state.nodes {
            let routes = match &node.state {
                AccountState::Active(session) => session.session().map(build_route_set_snapshot),
                _ => None,
            };
            let local_routes = match &node.state {
                AccountState::Active(session) => session
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

    /// Unified failure reporter: mark an Active node as Dead with doubled backoff.
    pub async fn report_failure(&self, account_name: &str) {
        let mut state = self.inner.lock().await;
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|node| node.account.name == account_name && matches!(node.state, AccountState::Active(_)))
        {
                        node.backoff = state::next_backoff(node.backoff, self.config.backoff_max);
                node.backoff_until = Some(Instant::now() + node.backoff);
                node.state = AccountState::Dead;
                tracing::warn!(
                    account = %account_name,
                    backoff_secs = node.backoff.as_secs(),
                    "active session marked dead after proxy failure"
                );
        }
    }

    fn spawn_background_maintenance_task(&self) {
        let interval = self.config.healthcheck_interval;
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
                        pool.maintenance_tick().await;
                    }
                }
            }
            maintenance.running.store(false, Ordering::Release);
        }));
    }

    fn build_keepalive_handle(
        &self,
        account_name: &str,
        session: &Session,
    ) -> Option<Arc<std::sync::Mutex<smelly_connect::KeepaliveHandle>>> {
        let target = self.config.keepalive_target.clone()?;
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
                        pool.report_failure(&account_name).await;
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

    /// Acquire a healthy session. Returns immediately with round-robin from Active pool,
    /// or waits briefly if nodes are connecting, or returns NoReadyNode.
    pub async fn acquire(&self) -> Result<PooledSession, PoolError> {
        // Fast path: round-robin from Active nodes
        {
            let mut state = self.inner.lock().await;
            let idx = next_selectable_index(&mut state, |node| {
                matches!(node.state, AccountState::Active(_))
            });
            if let Some(AccountState::Active(session)) = idx.map(|i| &state.nodes[i].state) {
                return Ok(session.as_ref().clone());
            }
        }

        // Slow path: if any node is Connecting, wait briefly for Notify
        let has_connecting = {
            let state = self.inner.lock().await;
            state
                .nodes
                .iter()
                .any(|node| matches!(node.state, AccountState::Connecting))
        };
        if has_connecting {
            let notify = {
                let state = self.inner.lock().await;
                Arc::clone(&state.notify)
            };
            let notified = notify.notified();
            tokio::select! {
                _ = notified => {
                    let mut state = self.inner.lock().await;
                    let idx = next_selectable_index(&mut state, |node| {
                        matches!(node.state, AccountState::Active(_))
                    });
                    if let Some(AccountState::Active(session)) = idx.map(|i| &state.nodes[i].state) {
                        return Ok(session.as_ref().clone());
                    }
                }
                _ = tokio::time::sleep(ACQUIRE_NOTIFY_TIMEOUT) => {}
            }
        }

        Err(PoolError::NoReadyNode)
    }

    /// Single maintenance tick. Called periodically and on startup.
    /// Returns JoinHandles for Phase 2 connect tasks so callers may await them.
    async fn maintenance_tick(&self) -> Vec<tokio::task::JoinHandle<()>> {
        let now = Instant::now();

        // Phase 1: time progression
        {
            let mut state = self.inner.lock().await;
            for node in &mut state.nodes {
                match node.state {
                    AccountState::Dead if node.backoff_until.is_some_and(|until| now >= until) => {
                        node.state = AccountState::Idle;
                        node.backoff_until = None;
                        tracing::info!(
                            account = %node.account.name,
                            "dead -> idle after backoff"
                        );
                    }
                    AccountState::Connecting
                        if node.backoff_until.is_some_and(|until| now >= until) =>
                    {
                        node.backoff =
                            state::next_backoff(node.backoff, self.config.backoff_max);
                        node.backoff_until = Some(now + node.backoff);
                        node.state = AccountState::Dead;
                        tracing::warn!(
                            account = %node.account.name,
                            backoff_secs = node.backoff.as_secs(),
                            "connecting timed out, -> dead"
                        );
                    }
                    _ => {}
                }
            }
        }

        // Phase 2: fill deficit
        let deficit = {
            let state = self.inner.lock().await;
            let active = self.active_count_locked(&state);
            let connecting = state
                .nodes
                .iter()
                .filter(|n| matches!(n.state, AccountState::Connecting))
                .count();
            let target = self.config.min_pool_size;
            if target > active + connecting {
                target - active - connecting
            } else {
                0
            }
        };

        let mut handles = Vec::new();
        for _ in 0..deficit {
            let (name, account, server) = {
                let mut state = self.inner.lock().await;
                let idx = state.nodes.iter_mut().enumerate().find(|(_, n)| {
                    matches!(n.state, AccountState::Idle)
                });
                match idx {
                    Some((_i, node)) => {
                        let name = node.account.name.clone();
                        let account = node.account.clone();
                        let server = self.config.server.clone();
                        if server.is_none() {
                            continue;
                        }
                        node.state = AccountState::Connecting;
                        node.backoff_until = Some(now + self.config.connect_timeout);
                        (name, account, server.unwrap())
                    }
                    None => break,
                }
            };

            let pool = Arc::new(self.clone_with_refcount(false));
            handles.push(tokio::spawn(async move {
                pool.recover_account_session(&name, &account, &server)
                    .await;
            }));
        }

        // Phase 3: health probe Active nodes (if keepalive_target is configured)
        if self.config.keepalive_target.is_some() {
            let targets: Vec<(String, Session)> = {
                let mut state = self.inner.lock().await;
                let mut out = Vec::new();
                for node in &mut state.nodes {
                    if node.probe_in_flight {
                        continue;
                    }
                    if let Some(live) = match &node.state {
                        AccountState::Active(session) => session.session().cloned(),
                        _ => None,
                    } {
                        node.probe_in_flight = true;
                        out.push((node.account.name.clone(), live));
                    }
                }
                out
            };

            if let Some(ref target) = self.config.keepalive_target {
                for (name, session) in targets {
                    let pool = self.clone_with_refcount(false);
                    let target = target.clone();
                    tokio::spawn(async move {
                        let result = probe_live_session_health(
                            &session,
                            smelly_connect::session::IcmpKeepAliveTarget::from(target),
                        )
                        .await;
                        if result.is_err() {
                            pool.report_failure(&name).await;
                        } else {
                            let mut state = pool.inner.lock().await;
                            if let Some(node) = state
                                .nodes
                                .iter_mut()
                                .find(|n| n.account.name == name)
                            {
                                node.probe_in_flight = false;
                            }
                        }
                    });
                }
            }
        }

        handles
    }

    /// Spawn a full recovery (login + session) for an account transitioning from
    /// Connecting to Active/Dead/Disabled.
    async fn recover_account_session(
        self: &Arc<Self>,
        name: &str,
        account: &AccountConfig,
        server: &str,
    ) {
        let result = connect_account(
            server,
            account,
            self.config.connect_timeout,
            ConnectAccountContext {
                local_route_overrides: &self.config.local_route_overrides,
                route_policy: self.config.route_policy,
                allow_all_routes: self.config.allow_all_routes,
                _keepalive_target: self.config.keepalive_target.as_deref(),
                server_cert_policy: self.config.server_cert_policy.clone(),
            },
        )
        .await;

        let mut state = self.inner.lock().await;
        match result {
            Ok(session) => {
                let pooled = self.wrap_live_session(name.to_string(), session);
                if let Some(node) = state
                    .nodes
                    .iter_mut()
                    .find(|n| n.account.name == name)
                {
                    node.state = AccountState::Active(Box::new(pooled));
                    node.backoff = self.config.backoff_base;
                    state.total_reconnections += 1;
                    tracing::info!(account = %name, "recover -> active");
                }
            }
            Err(err) => {
                if is_permanent_auth_failure(&err) {
                    if let Some(node) = state
                        .nodes
                        .iter_mut()
                        .find(|n| n.account.name == name)
                    {
                        node.state = AccountState::Disabled;
                        tracing::error!(account = %name, error = %err, "permanent auth failure, -> disabled");
                    }
                } else {
                    if let Some(node) = state
                        .nodes
                        .iter_mut()
                        .find(|n| n.account.name == name)
                    {
                node.backoff = state::next_backoff(node.backoff, self.config.backoff_max);
                        node.backoff_until = Some(Instant::now() + node.backoff);
                        node.state = AccountState::Dead;
                        tracing::warn!(
                            account = %name,
                            error = %err,
                            backoff_secs = node.backoff.as_secs(),
                            "recover failed, -> dead"
                        );
                    }
                }
            }
        }
        state.notify.notify_waiters();
        drop(state);
    }
}

fn is_permanent_auth_failure(error: &PoolError) -> bool {
    error
        .underlying_error()
        .is_some_and(smelly_connect::Error::is_permanent_auth_failure)
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
}

async fn connect_account(
    server: &str,
    account: &AccountConfig,
    timeout: Duration,
    ctx: ConnectAccountContext<'_>,
) -> Result<Session, PoolError> {
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
        .map_err(|_| PoolError::new("session connect timeout"))?
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
