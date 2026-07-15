use std::future::Future;
use std::sync::{Arc, Mutex, Weak};

use crate::error::{Error, IntegrationError};
use crate::proxy::http::ProxyHandle;
use crate::runtime::tasks::keepalive::KeepaliveHandle;
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

const DEFAULT_CONNECT_GATE_PERMITS: usize = 16;

pub(crate) struct SessionRuntime {
    _request_ip_tunnel: Option<smelly_tls::TunnelConnection>,
    _keepalive: Option<KeepaliveHandle>,
    connect_gate: Arc<Semaphore>,
    reqwest_proxy: AsyncMutex<Weak<SessionReqwestProxy>>,
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            _request_ip_tunnel: None,
            _keepalive: None,
            connect_gate: Arc::new(Semaphore::new(DEFAULT_CONNECT_GATE_PERMITS)),
            reqwest_proxy: AsyncMutex::new(Weak::new()),
        }
    }
}

impl SessionRuntime {
    pub(crate) fn new(
        request_ip_tunnel: Option<smelly_tls::TunnelConnection>,
        keepalive: Option<KeepaliveHandle>,
    ) -> Self {
        Self {
            _request_ip_tunnel: request_ip_tunnel,
            _keepalive: keepalive,
            connect_gate: Arc::new(Semaphore::new(DEFAULT_CONNECT_GATE_PERMITS)),
            reqwest_proxy: AsyncMutex::new(Weak::new()),
        }
    }

    pub(crate) async fn acquire_connect_permit(&self) -> OwnedSemaphorePermit {
        self.connect_gate
            .clone()
            .acquire_owned()
            .await
            .expect("connect gate semaphore closed")
    }

    pub(crate) async fn shared_reqwest_proxy<F, Fut>(
        &self,
        create: F,
    ) -> Result<Arc<SessionReqwestProxy>, Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<ProxyHandle, Error>>,
    {
        let mut cached = self.reqwest_proxy.lock().await;
        if let Some(proxy) = cached.upgrade() {
            return Ok(proxy);
        }

        let proxy = Arc::new(SessionReqwestProxy::new(create().await?)?);
        *cached = Arc::downgrade(&proxy);
        Ok(proxy)
    }
}

pub(crate) struct SessionReqwestProxy {
    proxy_url: reqwest::Url,
    handle: Mutex<Option<ProxyHandle>>,
}

impl SessionReqwestProxy {
    fn new(handle: ProxyHandle) -> Result<Self, Error> {
        let local_addr = handle.local_addr();
        let proxy_url = reqwest::Url::parse(&format!("http://{local_addr}")).map_err(|err| {
            Error::Integration(IntegrationError::ClientBuildFailed(err.to_string()))
        })?;

        Ok(Self {
            proxy_url,
            handle: Mutex::new(Some(handle)),
        })
    }

    pub(crate) fn proxy_url(&self) -> &reqwest::Url {
        &self.proxy_url
    }
}

impl Drop for SessionReqwestProxy {
    fn drop(&mut self) {
        let Some(handle) = self.handle.get_mut().ok().and_then(Option::take) else {
            return;
        };

        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = handle.shutdown().await;
            });
            return;
        }

        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let _ = handle.shutdown().await;
            });
        });
    }
}
