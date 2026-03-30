#[tokio::test]
async fn packet_device_forwards_frames_between_channels_and_stack() {
    let harness = smelly_connect::test_support::transport::packet_harness();
    harness.inject_from_vpn(vec![0, 1, 2, 3]).await;
    assert_eq!(harness.read_for_stack().await, vec![0, 1, 2, 3]);
}

#[tokio::test]
async fn packet_device_forwards_frames_from_stack_to_vpn() {
    let harness = smelly_connect::test_support::transport::packet_harness();
    harness.write_from_stack(vec![4, 5, 6, 7]).await;
    assert_eq!(harness.read_for_vpn().await, vec![4, 5, 6, 7]);
}

#[tokio::test]
async fn stack_can_create_outbound_tcp_stream_handle() {
    let harness = smelly_connect::test_support::transport::stack_harness();
    let _stream = harness.connect(("10.0.0.8", 443)).await.unwrap();
}

#[tokio::test]
async fn stack_can_create_outbound_udp_socket_handle() {
    let harness = smelly_connect::test_support::transport::stack_harness();
    let socket = harness.bind_udp().await.unwrap();
    assert!(socket.local_addr().unwrap().port() > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn packet_device_builds_real_smoltcp_transport() {
    let harness = smelly_connect::test_support::transport::packet_harness();
    let transport = smelly_connect::transport::netstack::build_transport_from_packet_device(
        harness.into_device(),
        "10.0.0.8".parse().unwrap(),
    )
    .unwrap();
    let _ = transport;
}

#[tokio::test]
async fn session_connect_tcp_returns_async_stream() {
    let harness = smelly_connect::test_support::session::login_harness();
    let session = harness.ready_session().await;
    let _stream = session.connect_tcp(("10.0.0.8", 443)).await.unwrap();
}

#[tokio::test]
async fn session_connect_tcp_preserves_timeout_as_structured_transport_error() {
    let session =
        smelly_connect::test_support::session::session_with_immediate_timeout_domain_match(
            "jwxt.sit.edu.cn",
            "10.0.0.8".parse().unwrap(),
        );
    match session.connect_tcp(("jwxt.sit.edu.cn", 443)).await {
        Err(smelly_connect::Error::Transport(
            smelly_connect::error::TransportError::ConnectTimedOut,
        )) => {}
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("expected timeout error"),
    }
}

#[tokio::test]
async fn session_bind_udp_returns_datagram_handle() {
    let harness = smelly_connect::test_support::session::login_harness();
    let session = harness.ready_session().await;
    let socket = session.bind_udp().await.unwrap();
    assert!(socket.local_addr().unwrap().port() > 0);
}

#[tokio::test]
async fn session_keepalive_task_invokes_transport_icmp_ping() {
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let session = smelly_connect::test_support::session::session_with_icmp_ping(counter.clone());
    let handle = session.spawn_icmp_keepalive_task(
        smelly_connect::session::IcmpKeepAliveTarget::Ip("10.0.0.8".parse().unwrap()),
        std::time::Duration::from_millis(20),
    );
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    handle.abort();
    assert!(
        counter.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "expected at least two icmp keepalive attempts"
    );
}

#[tokio::test]
async fn session_serializes_concurrent_connect_establishment() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let transport = smelly_connect::transport::TransportStack::new({
        let active = active.clone();
        let max_active = max_active.clone();
        move |_| {
            let active = active.clone();
            let max_active = max_active.clone();
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                let (client, _server) = tokio::io::duplex(1024);
                Ok(smelly_connect::transport::VpnStream::new(client))
            }
        }
    });

    let mut resources = smelly_connect::resource::ResourceSet::default();
    resources.domain_rules.insert(
        "jwxt.sit.edu.cn".to_string(),
        smelly_connect::resource::DomainRule {
            port_min: 443,
            port_max: 443,
            protocol: smelly_connect::RouteProtocol::Tcp,
        },
    );
    let mut system_dns = std::collections::HashMap::new();
    system_dns.insert(
        "jwxt.sit.edu.cn".to_string(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(210, 35, 66, 210)),
    );
    let session = smelly_connect::session::EasyConnectSession::new(
        "10.0.0.8".parse().unwrap(),
        resources,
        smelly_connect::resolver::SessionResolver::new(
            std::collections::HashMap::new(),
            None,
            system_dns,
        ),
        transport,
    );

    let first = {
        let session = session.clone();
        tokio::spawn(async move { session.connect_tcp(("jwxt.sit.edu.cn", 443)).await })
    };
    let second = {
        let session = session.clone();
        tokio::spawn(async move { session.connect_tcp(("jwxt.sit.edu.cn", 443)).await })
    };

    let _ = first.await.unwrap().unwrap();
    let _ = second.await.unwrap().unwrap();

    assert_eq!(max_active.load(Ordering::SeqCst), 1);
}
