use std::sync::Arc;

use crate::error::{BootstrapError, Error};
use crate::session::EasyConnectSession;

type SessionFactory = dyn Fn() -> Result<EasyConnectSession, Error> + Send + Sync + 'static;

pub struct EasyConnectConfig {
    pub server: String,
    pub username: String,
    pub password: String,
    pub(crate) session_factory: Option<Arc<SessionFactory>>,
}

impl EasyConnectConfig {
    pub fn new(
        server: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            server: server.into(),
            username: username.into(),
            password: password.into(),
            session_factory: None,
        }
    }

    pub async fn connect(self) -> Result<EasyConnectSession, Error> {
        match self.session_factory {
            Some(factory) => factory(),
            None => Err(Error::Bootstrap(BootstrapError::NotImplemented)),
        }
    }
}
