use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use crate::domain::route_policy::RoutePolicy;
use crate::resolver::SessionResolver;
use crate::resource::ResourceSet;
use crate::session::runtime::SessionRuntime;
use crate::session::LocalRouteOverrides;
use crate::transport::TransportStack;

#[derive(Clone)]
pub(crate) struct SessionInner {
    pub(crate) client_ip: Ipv4Addr,
    pub(crate) resources: ResourceSet,
    pub(crate) resolver: SessionResolver,
    pub(crate) transport: TransportStack,
    pub(crate) local_route_overrides: LocalRouteOverrides,
    pub(crate) route_policy: RoutePolicy,
    pub(crate) allow_all_routes: bool,
    pub(crate) legacy_data_plane: Option<LegacyDataPlaneConfig>,
    pub(crate) runtime: Arc<SessionRuntime>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct LegacyDataPlaneConfig {
    pub(crate) server_addr: SocketAddr,
    pub(crate) token: crate::protocol::DerivedToken,
    pub(crate) legacy_cipher_hint: Option<String>,
}
