use std::sync::Mutex;

use crate::runtime::tasks::keepalive::KeepaliveHandle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct SessionRuntime {
    legacy_tunnel: Mutex<Option<smelly_tls::TunnelConnection>>,
    keepalive: Mutex<Option<KeepaliveHandle>>,
    connect_gate: std::sync::Arc<Semaphore>,
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            legacy_tunnel: Mutex::new(None),
            keepalive: Mutex::new(None),
            connect_gate: std::sync::Arc::new(Semaphore::new(1)),
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
            connect_gate: std::sync::Arc::new(Semaphore::new(1)),
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
