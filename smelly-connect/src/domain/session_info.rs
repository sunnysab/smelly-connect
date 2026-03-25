use std::net::Ipv4Addr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    client_ip: Ipv4Addr,
}

impl SessionInfo {
    pub fn new(client_ip: Ipv4Addr) -> Self {
        Self { client_ip }
    }

    pub fn client_ip(&self) -> Ipv4Addr {
        self.client_ip
    }
}
