use std::sync::Mutex;

use crate::runtime::tasks::keepalive::KeepaliveHandle;

#[derive(Default)]
pub(crate) struct SessionRuntime {
    legacy_tunnel: Mutex<Option<smelly_tls::TunnelConnection>>,
    keepalive: Mutex<Option<KeepaliveHandle>>,
}

impl SessionRuntime {
    pub(crate) fn new(
        legacy_tunnel: Option<smelly_tls::TunnelConnection>,
        keepalive: Option<KeepaliveHandle>,
    ) -> Self {
        Self {
            legacy_tunnel: Mutex::new(legacy_tunnel),
            keepalive: Mutex::new(keepalive),
        }
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
