use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

#[cfg(any(test, debug_assertions))]
use crate::error::Error;
use crate::resolver::SessionResolver;
use crate::resource::ResourceSet;
use crate::session::runtime::SessionRuntime;
use crate::transport::TransportStack;

#[cfg(any(test, debug_assertions))]
pub(crate) type TransportRebuilder =
    dyn Fn() -> Result<TransportStack, Error> + Send + Sync + 'static;

#[derive(Clone)]
pub(crate) struct SessionInner {
    pub(crate) client_ip: Ipv4Addr,
    pub(crate) resources: ResourceSet,
    pub(crate) resolver: SessionResolver,
    pub(crate) transport: TransportStack,
    pub(crate) legacy_data_plane: Option<LegacyDataPlaneConfig>,
    pub(crate) runtime: Arc<SessionRuntime>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct LegacyDataPlaneConfig {
    pub(crate) server_addr: SocketAddr,
    pub(crate) token: crate::protocol::DerivedToken,
    pub(crate) legacy_cipher_hint: Option<String>,
    #[cfg(any(test, debug_assertions))]
    pub(crate) transport_rebuilder: Option<Arc<TransportRebuilder>>,
}
