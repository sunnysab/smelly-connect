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
