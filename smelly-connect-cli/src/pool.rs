use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct PooledSession {
    account_name: String,
}

impl PooledSession {
    pub fn account_name(&self) -> &str {
        &self.account_name
    }
}

#[derive(Debug, Clone)]
pub struct AccountFailure {
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum AccountState {
    Configured,
    Connecting,
    Ready(PooledSession),
    Failed(AccountFailure),
}

#[derive(Debug, Clone)]
struct AccountNode {
    name: String,
    state: AccountState,
    flaky_retry: bool,
}

#[derive(Debug, Default)]
struct PoolState {
    nodes: Vec<AccountNode>,
    cursor: usize,
}

#[derive(Clone)]
pub struct SessionPool {
    inner: Arc<Mutex<PoolState>>,
    retry_delay: Duration,
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
                AccountState::Ready(PooledSession {
                    account_name: name.clone(),
                })
            } else {
                AccountState::Configured
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
        }
    }

    pub async fn from_named_ready_accounts<const N: usize>(names: [&str; N]) -> Self {
        let nodes = names
            .into_iter()
            .map(|name| AccountNode {
                name: name.to_string(),
                state: AccountState::Ready(PooledSession {
                    account_name: name.to_string(),
                }),
                flaky_retry: false,
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(PoolState { nodes, cursor: 0 })),
            retry_delay: Duration::from_secs(1),
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
                    AccountState::Ready(PooledSession {
                        account_name: name.to_string(),
                    }),
                ),
                Ok(name) => (name.to_string(), AccountState::Configured),
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
        }
    }

    pub async fn from_flaky_account_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                nodes: vec![AccountNode {
                    name: "acct-01".to_string(),
                    state: AccountState::Ready(PooledSession {
                        account_name: "acct-01".to_string(),
                    }),
                    flaky_retry: true,
                }],
                cursor: 0,
            })),
            retry_delay: Duration::from_millis(100),
        }
    }

    pub async fn ready_count(&self) -> usize {
        let state = self.inner.lock().await;
        state
            .nodes
            .iter()
            .filter(|node| matches!(node.state, AccountState::Ready(_)))
            .count()
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
                AccountState::Ready(session) => Some((idx, session.clone())),
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
            .find(|node| matches!(node.state, AccountState::Configured))
        {
            node.state = AccountState::Ready(PooledSession {
                account_name: node.name.clone(),
            });
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
                should_retry = flaky_retry.then_some(name);
            }
        }

        if let Some(name) = should_retry {
            let inner = Arc::clone(&self.inner);
            let retry_delay = self.retry_delay;
            tokio::spawn(async move {
                tokio::time::sleep(retry_delay).await;
                let mut state = inner.lock().await;
                if let Some(node) = state.nodes.iter_mut().find(|node| node.name == name) {
                    node.state = AccountState::Ready(PooledSession {
                        account_name: node.name.clone(),
                    });
                }
            });
        }
    }
}
