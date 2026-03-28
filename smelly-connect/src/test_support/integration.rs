use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::resolver::SessionResolver;
use crate::resource::{DomainRule, IpRule, ResourceSet};
use crate::session::EasyConnectSession;
use crate::transport::{TransportStack, VpnStream};

pub struct ReqwestHarness {
    pub session: EasyConnectSession,
}

impl ReqwestHarness {
    pub async fn get_with(&self, client: reqwest::Client, url: &str) -> String {
        client.get(url).send().await.unwrap().text().await.unwrap()
    }
}

pub async fn reqwest_harness() -> ReqwestHarness {
    let upstream = spawn_http_upstream().await;
    ReqwestHarness {
        session: ready_session(upstream),
    }
}

async fn spawn_http_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 1024];
        let _ = socket.read(&mut buf).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    addr
}

fn ready_session(http_upstream: SocketAddr) -> EasyConnectSession {
    let host = "intranet.zju.edu.cn";
    let resolved_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let mut resources = ResourceSet::default();
    resources.domain_rules.insert(
        host.to_string(),
        DomainRule {
            port_min: 80,
            port_max: 80,
            protocol: crate::RouteProtocol::All,
        },
    );
    resources.ip_rules.push(IpRule {
        ip_min: resolved_ip,
        ip_max: resolved_ip,
        port_min: 1,
        port_max: 65535,
        protocol: crate::RouteProtocol::All,
    });

    let mut system_dns = HashMap::new();
    system_dns.insert(host.to_string(), resolved_ip);

    let transport = TransportStack::new(move |_| {
        let upstream = http_upstream;
        async move {
            let stream = TcpStream::connect(upstream).await?;
            Ok(VpnStream::new(stream))
        }
    });

    EasyConnectSession::new(
        Ipv4Addr::new(10, 0, 0, 8),
        resources,
        SessionResolver::new(HashMap::new(), None, system_dns),
        transport,
    )
}
