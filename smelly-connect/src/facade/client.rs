use crate::auth::CaptchaHandler;
use crate::config::EasyConnectConfig;
use crate::error::{ControlPlaneError, Error};
use crate::facade::session::Session;

pub struct EasyConnectClient {
    config: EasyConnectConfig,
}

pub struct EasyConnectClientBuilder {
    server: String,
    username: Option<String>,
    password: Option<String>,
    base_url: Option<String>,
    captcha_handler: Option<CaptchaHandler>,
}

impl EasyConnectClient {
    pub fn builder(server: impl Into<String>) -> EasyConnectClientBuilder {
        EasyConnectClientBuilder {
            server: server.into(),
            username: None,
            password: None,
            base_url: None,
            captcha_handler: None,
        }
    }

    pub async fn connect(&self) -> Result<Session, Error> {
        self.config.clone().connect().await
    }
}

impl EasyConnectClientBuilder {
    pub fn credentials(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn with_captcha_handler(mut self, captcha_handler: CaptchaHandler) -> Self {
        self.captcha_handler = Some(captcha_handler);
        self
    }

    pub fn build(self) -> Result<EasyConnectClient, Error> {
        let username = self.username.ok_or_else(|| {
            Error::ControlPlane(ControlPlaneError::AuthFlowFailed(
                "missing EasyConnect username".to_string(),
            ))
        })?;
        let password = self.password.ok_or_else(|| {
            Error::ControlPlane(ControlPlaneError::AuthFlowFailed(
                "missing EasyConnect password".to_string(),
            ))
        })?;

        let mut config = EasyConnectConfig::new(self.server, username, password);
        if let Some(base_url) = self.base_url {
            config = config.with_base_url(base_url);
        }
        if let Some(captcha_handler) = self.captcha_handler {
            config = config.with_captcha_handler(captcha_handler);
        }

        Ok(EasyConnectClient { config })
    }
}
