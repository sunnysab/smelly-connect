use std::collections::HashMap;
use std::net::IpAddr;

use crate::error::ResolveError;

#[derive(Clone)]
pub struct SessionResolver {
    static_dns: HashMap<String, IpAddr>,
    remote_dns: Option<HashMap<String, IpAddr>>,
    system_dns: HashMap<String, IpAddr>,
}

impl SessionResolver {
    pub fn new(
        static_dns: HashMap<String, IpAddr>,
        remote_dns: Option<HashMap<String, IpAddr>>,
        system_dns: HashMap<String, IpAddr>,
    ) -> Self {
        Self {
            static_dns,
            remote_dns,
            system_dns,
        }
    }

    pub async fn resolve_for_vpn(&self, host: &str) -> Result<IpAddr, ResolveError> {
        if let Some(ip) = self.static_dns.get(host) {
            return Ok(*ip);
        }
        if let Some(remote_dns) = &self.remote_dns
            && let Some(ip) = remote_dns.get(host)
        {
            return Ok(*ip);
        }
        if let Some(ip) = self.system_dns.get(host) {
            return Ok(*ip);
        }

        tokio::net::lookup_host((host, 0))
            .await
            .map_err(|_| ResolveError::NoRecordFound)?
            .find_map(|addr| match addr.ip() {
                IpAddr::V4(ip) => Some(IpAddr::V4(ip)),
                IpAddr::V6(_) => None,
            })
            .ok_or(ResolveError::NoRecordFound)
    }
}

pub mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    use crate::resolver::SessionResolver;

    pub fn resolver_with_failing_remote() -> SessionResolver {
        let mut system_dns = HashMap::new();
        system_dns.insert(
            "libdb.zju.edu.cn".to_string(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)),
        );
        SessionResolver::new(HashMap::new(), Some(HashMap::new()), system_dns)
    }
}
