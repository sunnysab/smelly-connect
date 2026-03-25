use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use smelly_connect::{CaptchaError, CaptchaHandler, EasyConnectClient, Session};
use tokio::sync::Mutex;

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
    Failed(AccountFailure),
}

#[derive(Clone)]
struct AccountNode {
    name: String,
    state: AccountState,
    flaky_retry: bool,
}

#[derive(Default)]
struct PoolState {
    nodes: Vec<AccountNode>,
    cursor: usize,
}

#[derive(Clone)]
pub struct SessionPool {
    inner: Arc<Mutex<PoolState>>,
    retry_delay: Duration,
    server: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PoolError {
    message: String,
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
                name,
                state,
                flaky_retry: false,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            retry_delay: Duration::from_secs(1),
            server: None,
        }
    }

    pub async fn from_named_ready_accounts<const N: usize>(names: [&str; N]) -> Self {
        let nodes = names
            .into_iter()
            .map(|name| AccountNode {
                name: name.to_string(),
                state: AccountState::Ready(
                    PooledSession {
                        account_name: name.to_string(),
                        session: None,
                    }
                    .into(),
                ),
                flaky_retry: false,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            retry_delay: Duration::from_secs(1),
            server: None,
        }
    }

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
                    AccountState::Failed(AccountFailure {
                        message: message.to_string(),
                    }),
                ),
            };
            nodes.push(AccountNode {
                name,
                state,
                flaky_retry: false,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            retry_delay: Duration::from_secs(1),
            server: None,
        }
    }

    pub async fn from_failed_accounts(total: usize) -> Self {
        let mut nodes = Vec::new();
        for idx in 0..total {
            nodes.push(AccountNode {
                name: format!("failed-{:02}", idx + 1),
                state: AccountState::Failed(AccountFailure {
                    message: "not ready".to_string(),
                }),
                flaky_retry: false,
            });
        }
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            retry_delay: Duration::from_secs(1),
            server: None,
        }
    }

    pub async fn from_flaky_account_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    name: "acct-01".to_string(),
                    state: AccountState::Ready(
                        PooledSession {
                            account_name: "acct-01".to_string(),
                            session: None,
                        }
                        .into(),
                    ),
                    flaky_retry: true,
                }],
                cursor: 0,
            })),
            retry_delay: Duration::from_millis(100),
            server: None,
        }
    }

    pub async fn from_config(cfg: &AppConfig) -> Result<Self, PoolError> {
        tracing::info!(
            accounts = cfg.accounts.len(),
            prewarm = cfg.pool.prewarm,
            "pool prewarm start"
        );
        let mut nodes = Vec::new();
        for account in &cfg.accounts {
            nodes.push(AccountNode {
                name: account.name.clone(),
                state: AccountState::Configured(account.clone()),
                flaky_retry: false,
            });
        }

        let pool = Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            retry_delay: Duration::from_secs(cfg.pool.healthcheck_interval_secs.max(1)),
            server: Some(cfg.vpn.server.clone()),
        };

        pool.prewarm(cfg.pool.prewarm).await;
        let ready = pool.ready_count().await;
        tracing::info!(
            configured = cfg.accounts.len(),
            ready,
            "pool startup summary"
        );
        if ready == 0 {
            tracing::error!("no ready session after prewarm");
            return Err(PoolError::new("no ready session after prewarm"));
        }
        Ok(pool)
    }

    pub async fn ready_count(&self) -> usize {
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .filter(|node| matches!(node.state, AccountState::Ready(_)))
            .count()
    }

    pub async fn state_summary_for_test(&self) -> String {
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .map(|node| {
                let label = match node.state {
                    AccountState::Configured(_) => "Configured",
                    AccountState::Connecting => "Connecting",
                    AccountState::Ready(_) => "Ready",
                    AccountState::Failed(_) => "Failed",
                };
                format!("{}:{label}", node.name)
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    pub async fn has_selectable_nodes_for_test(&self) -> bool {
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .any(|node| matches!(node.state, AccountState::Ready(_)))
    }

    pub async fn next_account_name(&self) -> Result<String, PoolError> {
        Ok(self.next_session().await?.account_name().to_string())
    }

    pub async fn next_session(&self) -> Result<PooledSession, PoolError> {
        let mut state = self.inner.lock().await;
        let ready: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| match &node.state {
                AccountState::Ready(session) => Some((idx, session.as_ref().clone())),
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

    pub async fn force_one_failure_for_test(&self) {
        let mut should_retry = None;
        {
            let mut state = self.inner.lock().await;
            if let Some(node) = state
                .nodes
                .iter_mut()
                .find(|node| matches!(node.state, AccountState::Ready(_)))
            {
                let name = node.name.clone();
                let flaky_retry = node.flaky_retry;
                node.state = AccountState::Failed(AccountFailure {
                    message: "forced failure".to_string(),
                });
                tracing::warn!(account = %name, "account forced into failed state");
                should_retry = flaky_retry.then_some(name);
            }
        }

        if let Some(name) = should_retry {
            let inner = Arc::clone(&self.inner);
            let retry_delay = self.retry_delay;
            tokio::spawn(async move {
                tracing::warn!(account = %name, delay_ms = retry_delay.as_millis(), "retrying account after fixed-delay backoff");
                tokio::time::sleep(retry_delay).await;
                let mut state = inner.lock().await;
                if let Some(node) = state.nodes.iter_mut().find(|node| node.name == name) {
                    node.state = AccountState::Ready(
                        PooledSession {
                            account_name: node.name.clone(),
                            session: None,
                        }
                        .into(),
                    );
                    tracing::info!(account = %node.name, "account ready");
                }
            });
        }
    }

    pub async fn next_live_session(&self) -> Result<(String, Session), PoolError> {
        if let Some(ready) = self.next_ready_with_session().await? {
            return Ok(ready);
        }

        self.connect_one_configured().await?;

        if let Some(ready) = self.next_ready_with_session().await? {
            return Ok(ready);
        }

        Err(PoolError::new("no ready session"))
    }

    async fn prewarm(&self, count: usize) {
        for _ in 0..count {
            let _ = self.connect_one_configured().await;
        }
    }

    async fn next_ready_with_session(&self) -> Result<Option<(String, Session)>, PoolError> {
        let mut state = self.inner.lock().await;
        let ready: Vec<_> = state
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| match &node.state {
                AccountState::Ready(session) => session
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
            let AccountState::Configured(account) = &state.nodes[idx].state else {
                unreachable!();
            };
            let account = account.clone();
            let name = state.nodes[idx].name.clone();
            state.nodes[idx].state = AccountState::Connecting;
            (name, account, server)
        };

        match connect_account(&server, &account, self.retry_delay).await {
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
                    node.state = AccountState::Failed(AccountFailure {
                        message: err.to_string(),
                    });
                }
                tracing::warn!(account = %account.name, error = %err, "account prewarm failed");
                Err(err)
            }
        }
    }
}

async fn connect_account(
    server: &str,
    account: &AccountConfig,
    timeout: Duration,
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

    tokio::time::timeout(timeout, client.connect())
        .await
        .map_err(|_| PoolError::new("session connect timeout"))?
        .map_err(|err| PoolError::new(format!("{err:?}")))
}
