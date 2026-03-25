use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use crate::auth::CaptchaHandler;
use crate::error::Error;
use crate::runtime::control_plane::{ControlPlaneState, run_control_plane};
use crate::session::{EasyConnectSession, IcmpKeepAliveTarget};
use crate::resolver::SessionResolver;

type SessionFactory = dyn Fn() -> Result<EasyConnectSession, Error> + Send + Sync + 'static;
type SessionBootstrap =
    dyn Fn(ControlPlaneState) -> Result<EasyConnectSession, Error> + Send + Sync + 'static;

#[derive(Clone)]
struct IcmpKeepAliveConfig {
    target: IcmpKeepAliveTarget,
    interval: Duration,
}

#[derive(Clone)]
pub struct EasyConnectConfig {
    pub server: String,
    pub username: String,
    pub password: String,
    pub(crate) base_url: Option<String>,
    pub(crate) captcha_handler: Option<CaptchaHandler>,
    pub(crate) session_factory: Option<Arc<SessionFactory>>,
    pub(crate) session_bootstrap: Option<Arc<SessionBootstrap>>,
    icmp_keepalive: Option<IcmpKeepAliveConfig>,
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
            icmp_keepalive: None,
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

    pub fn with_icmp_keepalive(mut self, target: impl Into<String>) -> Self {
        let target = target.into();
        let target = match target.parse() {
            Ok(ip) => IcmpKeepAliveTarget::Ip(ip),
            Err(_) => IcmpKeepAliveTarget::Host(target),
        };
        self.icmp_keepalive = Some(IcmpKeepAliveConfig {
            target,
            interval: Duration::from_secs(60),
        });
        self
    }

    pub fn with_icmp_keepalive_interval(mut self, interval: Duration) -> Self {
        if let Some(keepalive) = self.icmp_keepalive.as_mut() {
            keepalive.interval = interval;
        }
        self
    }

    pub async fn connect(self) -> Result<EasyConnectSession, Error> {
        if let Some(factory) = self.session_factory {
            return factory();
        }

        let state = run_control_plane(&self).await?;
        match self.session_bootstrap {
            Some(bootstrap) => bootstrap(state),
            None => self.default_bootstrap(state).await,
        }
    }

    pub(crate) fn control_base_url(&self) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| format!("https://{}", self.server))
    }

    async fn default_bootstrap(self, state: ControlPlaneState) -> Result<EasyConnectSession, Error> {
        let token = crate::auth::control::request_token(&self.server, &state.authorized_twfid)?;
        let server_addr = crate::auth::control::resolve_server_addr(&self.server)?;
        let (client_ip, request_ip_tunnel) = crate::auth::control::request_ip_via_tunnel_with_conn(
            server_addr,
            &token,
            state.legacy_cipher_hint.as_deref(),
        )
        .await?;

        let mut system_dns = std::collections::HashMap::new();
        for (host, resolved) in &state.resources.static_dns {
            system_dns.insert(host.clone(), *resolved);
        }

        let device = crate::auth::control::spawn_legacy_packet_device(
            server_addr,
            &token,
            client_ip,
            state.legacy_cipher_hint.as_deref(),
        )
        .await?;
        tokio::spawn(async move {
            let _request_ip_tunnel = request_ip_tunnel;
            pending::<()>().await;
        });
        let transport =
            crate::transport::netstack::build_transport_from_packet_device(device, client_ip)
                .map_err(|err| {
                    Error::Transport(crate::error::TransportError::ConnectFailed(err.to_string()))
                })?;
        let session = EasyConnectSession::new(
            client_ip,
            state.resources,
            SessionResolver::new(std::collections::HashMap::new(), None, system_dns),
            transport,
        )
        .with_legacy_data_plane(server_addr, token, state.legacy_cipher_hint);
        if let Some(keepalive) = &self.icmp_keepalive {
            std::mem::drop(
                session.spawn_icmp_keepalive_task(keepalive.target.clone(), keepalive.interval),
            );
        }
        Ok(session)
    }
}
