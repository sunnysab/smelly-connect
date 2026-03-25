use std::sync::Arc;

use crate::auth::{CaptchaHandler, ControlPlaneState, run_control_plane};
use crate::error::{BootstrapError, Error};
use crate::session::EasyConnectSession;

type SessionFactory = dyn Fn() -> Result<EasyConnectSession, Error> + Send + Sync + 'static;
type SessionBootstrap =
    dyn Fn(ControlPlaneState) -> Result<EasyConnectSession, Error> + Send + Sync + 'static;

pub struct EasyConnectConfig {
    pub server: String,
    pub username: String,
    pub password: String,
    pub(crate) base_url: Option<String>,
    pub(crate) captcha_handler: Option<CaptchaHandler>,
    pub(crate) session_factory: Option<Arc<SessionFactory>>,
    pub(crate) session_bootstrap: Option<Arc<SessionBootstrap>>,
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
            base_url: None,
            captcha_handler: None,
            session_factory: None,
            session_bootstrap: None,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn with_captcha_handler(mut self, captcha_handler: CaptchaHandler) -> Self {
        self.captcha_handler = Some(captcha_handler);
        self
    }

    pub fn with_session_bootstrap<F>(mut self, bootstrap: F) -> Self
    where
        F: Fn(ControlPlaneState) -> Result<EasyConnectSession, Error> + Send + Sync + 'static,
    {
        self.session_bootstrap = Some(Arc::new(bootstrap));
        self
    }

    pub async fn connect(self) -> Result<EasyConnectSession, Error> {
        if let Some(factory) = self.session_factory {
            return factory();
        }

        let state = run_control_plane(&self).await?;
        match self.session_bootstrap {
            Some(bootstrap) => bootstrap(state),
            None => Err(Error::Bootstrap(BootstrapError::NotImplemented)),
        }
    }

    pub(crate) fn control_base_url(&self) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| format!("https://{}", self.server))
    }
}
