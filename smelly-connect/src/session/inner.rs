use std::net::Ipv4Addr;
use std::sync::Arc;

use crate::domain::route_policy::RoutePolicy;
use crate::resolver::SessionResolver;
use crate::resource::ResourceSet;
use crate::session::LocalRouteOverrides;
use crate::session::runtime::SessionRuntime;
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
    pub(crate) runtime: Arc<SessionRuntime>,
}
