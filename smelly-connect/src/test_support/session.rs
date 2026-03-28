use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::duplex;
use tokio::net::UdpSocket;

use crate::config::EasyConnectConfig;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::resolver::SessionResolver;
use crate::session::{EasyConnectSession, IcmpKeepAliveTarget};
use crate::transport::{TransportStack, VpnStream, VpnUdpSocket};

pub fn fake_session_without_match() -> EasyConnectSession {
    EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        ResourceSet::default(),
        SessionResolver::new(HashMap::new(), None, HashMap::new()),
        ready_transport(),
    )
}

pub fn fake_session_without_match_with_transport(transport: TransportStack) -> EasyConnectSession {
    EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        ResourceSet::default(),
        SessionResolver::new(HashMap::new(), None, HashMap::new()),
        transport,
    )
}

pub struct LoginHarness {
    host: Arc<str>,
    ip: Ipv4Addr,
}

impl LoginHarness {
    pub fn config(&self) -> EasyConnectConfig {
        let host = Arc::clone(&self.host);
        let ip = self.ip;
        let mut config = EasyConnectConfig::new("rvpn.example.com", "user", "pass");
        config.session_factory = Some(Arc::new(move || Ok(ready_session(host.as_ref(), ip))));
        config
    }

    pub async fn ready_session(&self) -> EasyConnectSession {
        ready_session(self.host.as_ref(), self.ip)
    }
}

pub fn login_harness() -> LoginHarness {
    LoginHarness {
        host: Arc::<str>::from("libdb.zju.edu.cn"),
        ip: Ipv4Addr::new(10, 0, 0, 8),
    }
}

#[allow(dead_code)]
pub fn session_with_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
    ready_session(host, ip)
}

pub fn session_with_failing_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
    let (resources, system) = matched_resources(host, ip);
    EasyConnectSession::new(
        ip,
        resources,
        SessionResolver::new(HashMap::new(), None, system),
        EasyConnectSession::failing_transport("forced live connect failure"),
    )
}

pub fn session_with_slow_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
    let (resources, system) = matched_resources(host, ip);
    let transport = TransportStack::new(|_| async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "forced slow connect",
        ))
    });

    EasyConnectSession::new(
        ip,
        resources,
        SessionResolver::new(HashMap::new(), None, system),
        transport,
    )
}

pub fn session_with_immediate_timeout_domain_match(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
    let (resources, system) = matched_resources(host, ip);
    let transport = TransportStack::new(|_| async {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "forced immediate timeout",
        ))
    });

    EasyConnectSession::new(
        ip,
        resources,
        SessionResolver::new(HashMap::new(), None, system),
        transport,
    )
}

pub fn session_with_owned_keepalive(
    counter: Arc<AtomicUsize>,
    interval: Duration,
) -> EasyConnectSession {
    let transport = ready_transport().with_icmp_pinger(move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    let session = EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        ResourceSet::default(),
        SessionResolver::new(HashMap::new(), None, HashMap::new()),
        transport,
    );
    let keepalive = session.start_icmp_keepalive(
        IcmpKeepAliveTarget::Ip(Ipv4Addr::new(10, 0, 0, 8)),
        interval,
    );
    session.with_runtime_resources(None, Some(keepalive))
}

pub fn session_with_icmp_ping(counter: Arc<AtomicUsize>) -> EasyConnectSession {
    let transport = ready_transport().with_icmp_pinger(move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });

    EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        ResourceSet::default(),
        SessionResolver::new(HashMap::new(), None, HashMap::new()),
        transport,
    )
}

pub fn session_with_icmp_result(success: bool) -> EasyConnectSession {
    let transport = ready_transport().with_icmp_pinger(move |_| async move {
        if success {
            Ok(())
        } else {
            Err(io::Error::other("forced icmp failure"))
        }
    });

    EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        ResourceSet::default(),
        SessionResolver::new(HashMap::new(), None, HashMap::new()),
        transport,
    )
}

pub fn session_with_delayed_icmp_result(
    success: bool,
    delay: Duration,
    counter: Arc<AtomicUsize>,
) -> EasyConnectSession {
    let transport = ready_transport().with_icmp_pinger(move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            if success {
                Ok(())
            } else {
                Err(io::Error::other("forced delayed icmp failure"))
            }
        }
    });

    EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        ResourceSet::default(),
        SessionResolver::new(HashMap::new(), None, HashMap::new()),
        transport,
    )
}

pub fn session_with_failing_domain_match_and_delayed_icmp(
    host: &str,
    ip: Ipv4Addr,
    delay: Duration,
    counter: Arc<AtomicUsize>,
) -> EasyConnectSession {
    let (resources, system) = matched_resources(host, ip);
    let transport = EasyConnectSession::failing_transport("forced live connect failure")
        .with_icmp_pinger(move |_| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                Err(io::Error::other("forced delayed icmp failure"))
            }
        });

    EasyConnectSession::new(
        ip,
        resources,
        SessionResolver::new(HashMap::new(), None, system),
        transport,
    )
}

fn matched_resources(host: &str, ip: Ipv4Addr) -> (ResourceSet, HashMap<String, IpAddr>) {
    let mut resources = ResourceSet::default();
    resources.domain_rules.insert(
        host.to_string(),
        DomainRule {
            port_min: 1,
            port_max: 65535,
            protocol: crate::RouteProtocol::All,
        },
    );
    resources.ip_rules.push(IpRule {
        ip_min: IpAddr::V4(ip),
        ip_max: IpAddr::V4(ip),
        port_min: 1,
        port_max: 65535,
        protocol: crate::RouteProtocol::All,
    });
    resources.static_dns.insert(host.to_string(), IpAddr::V4(ip));

    let mut system = HashMap::new();
    system.insert(host.to_string(), IpAddr::V4(ip));
    (resources, system)
}

fn ready_session(host: &str, ip: Ipv4Addr) -> EasyConnectSession {
    let (resources, system) = matched_resources(host, ip);
    EasyConnectSession::new(
        ip,
        resources,
        SessionResolver::new(HashMap::new(), None, system),
        ready_transport(),
    )
}

fn ready_transport() -> TransportStack {
    TransportStack::new(|_| async {
        let (client, _server) = duplex(1024);
        Ok(VpnStream::new(client))
    })
    .with_udp_binder(|| async {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        Ok(VpnUdpSocket::new(socket))
    })
}
