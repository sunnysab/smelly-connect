use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak};

use crate::error::{Error, IntegrationError};
use crate::proxy::http::ProxyHandle;
use crate::runtime::tasks::keepalive::KeepaliveHandle;
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

const DEFAULT_CONNECT_GATE_PERMITS: usize = 16;

pub(crate) struct SessionRuntime {
    legacy_tunnel: Mutex<Option<smelly_tls::TunnelConnection>>,
    keepalive: Mutex<Option<KeepaliveHandle>>,
    connect_gate: std::sync::Arc<Semaphore>,
    reqwest_proxy: AsyncMutex<Weak<SessionReqwestProxy>>,
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            legacy_tunnel: Mutex::new(None),
            keepalive: Mutex::new(None),
            connect_gate: std::sync::Arc::new(Semaphore::new(DEFAULT_CONNECT_GATE_PERMITS)),
            reqwest_proxy: AsyncMutex::new(Weak::new()),
        }
    }
}

impl SessionRuntime {
    pub(crate) fn new(
        legacy_tunnel: Option<smelly_tls::TunnelConnection>,
        keepalive: Option<KeepaliveHandle>,
    ) -> Self {
        Self {
            legacy_tunnel: Mutex::new(legacy_tunnel),
            keepalive: Mutex::new(keepalive),
            connect_gate: std::sync::Arc::new(Semaphore::new(DEFAULT_CONNECT_GATE_PERMITS)),
            reqwest_proxy: AsyncMutex::new(Weak::new()),
        }
    }

    pub(crate) fn take_legacy_tunnel(&self) -> Option<smelly_tls::TunnelConnection> {
        self.legacy_tunnel
            .lock()
            .expect("legacy tunnel mutex poisoned")
            .take()
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

impl Drop for SessionRuntime {
    fn drop(&mut self) {
        if let Ok(legacy_tunnel) = self.legacy_tunnel.get_mut() {
            let _ = legacy_tunnel.take();
        }
        if let Ok(keepalive) = self.keepalive.get_mut() {
            let _ = keepalive.take();
        }
    }
}

pub(crate) struct SessionReqwestProxy {
    local_addr: SocketAddr,
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
            local_addr,
            proxy_url,
            handle: Mutex::new(Some(handle)),
        })
    }

    pub(crate) fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub(crate) fn proxy_url(&self) -> &reqwest::Url {
        &self.proxy_url
    }

    /// Shut down the proxy handle.  Idempotent — the second call is a no-op.
    #[allow(dead_code)]
    pub(crate) async fn shutdown(&self) {
        let handle = self
            .handle
            .lock()
            .expect("reqwest proxy mutex poisoned")
            .take();
        if let Some(handle) = handle {
            let _ = handle.shutdown().await;
        }
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
