use std::net::{Ipv4Addr, SocketAddr};

use crate::kernel::tunnel::DerivedToken;
use crate::resource::ResourceSet;

#[derive(Clone)]
pub struct ControlPlaneState {
    pub authorized_twfid: String,
    pub legacy_cipher_hint: Option<String>,
    pub resources: ResourceSet,
    pub token: Option<DerivedToken>,
}

#[derive(Clone)]
pub struct TunnelBootstrap {
    pub server_addr: SocketAddr,
    pub client_ip: Ipv4Addr,
    pub token: DerivedToken,
    pub legacy_cipher_hint: Option<String>,
}

#[derive(Clone)]
pub struct AuthenticatedSessionSeed {
    pub session_cookie: String,
    pub resources: ResourceSet,
    pub tunnel_bootstrap: TunnelBootstrap,
}
