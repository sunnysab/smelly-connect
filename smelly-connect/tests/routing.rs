#[tokio::test]
async fn routing_rejects_non_resource_targets_by_default() {
    let session = smelly_connect::test_support::session::fake_session_without_match();
    let err = session
        .plan_tcp_connect(("example.com", 443))
        .await
        .unwrap_err();
    assert!(matches!(err, smelly_connect::Error::RouteDecision(_)));
}

#[tokio::test]
async fn allow_all_bypasses_target_not_allowed_for_domains_and_ips() {
    let session =
        smelly_connect::test_support::session::fake_session_without_match().with_allow_all_routes(true);

    let domain_route = session
        .plan_tcp_connect(("example.com", 443))
        .await
        .unwrap();
    assert!(matches!(
        domain_route,
        smelly_connect::session::RoutePlan::VpnResolved(_)
    ));

    let ip_route = session.plan_tcp_connect(("172.24.9.11", 80)).await.unwrap();
    assert!(matches!(
        ip_route,
        smelly_connect::session::RoutePlan::VpnResolved(addr)
            if addr == "172.24.9.11:80".parse().unwrap()
    ));
}

#[tokio::test]
async fn config_connect_builds_session_with_client_ip() {
    let harness = smelly_connect::test_support::session::login_harness();
    let session = harness.config().connect().await.unwrap();
    assert_eq!(session.client_ip().to_string(), "10.0.0.8");
}

#[tokio::test]
async fn routing_allows_domain_resources_even_when_resolved_ip_is_not_in_ip_rules() {
    let mut resources = smelly_connect::resource::ResourceSet::default();
    resources.domain_rules.insert(
        "jwxt.sit.edu.cn".to_string(),
        smelly_connect::resource::DomainRule {
            port_min: 443,
            port_max: 443,
            protocol: smelly_connect::RouteProtocol::All,
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
        smelly_connect::session::EasyConnectSession::failing_transport("unused"),
    );

    let route = session
        .plan_tcp_connect(("jwxt.sit.edu.cn", 443))
        .await
        .unwrap();
    assert!(matches!(
        route,
        smelly_connect::session::RoutePlan::VpnResolved(addr)
            if addr == "210.35.66.210:443".parse().unwrap()
    ));
}

#[tokio::test]
async fn routing_allows_direct_single_ip_resources() {
    let resources = smelly_connect::resource::parse_resources(
        r#"
<Resource>
  <Rcs>
    <Rc type="1" proto="-1" host="210.35.66.210" port="443~443" />
  </Rcs>
  <Dns data="" dnsserver="10.10.0.21" />
</Resource>
"#,
    )
    .unwrap();

    let session = smelly_connect::session::EasyConnectSession::new(
        "10.0.0.8".parse().unwrap(),
        resources,
        smelly_connect::resolver::SessionResolver::new(
            std::collections::HashMap::new(),
            None,
            std::collections::HashMap::new(),
        ),
        smelly_connect::session::EasyConnectSession::failing_transport("unused"),
    );

    let route = session
        .plan_tcp_connect(("210.35.66.210", 443))
        .await
        .unwrap();
    assert!(matches!(
        route,
        smelly_connect::session::RoutePlan::VpnResolved(addr)
            if addr == "210.35.66.210:443".parse().unwrap()
    ));
}

#[tokio::test]
async fn local_route_overrides_allow_domain_and_ip_targets() {
    let mut system_dns = std::collections::HashMap::new();
    system_dns.insert(
        "portal.foo.edu.cn".to_string(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 8)),
    );
    let session = smelly_connect::session::EasyConnectSession::new(
        "10.0.0.8".parse().unwrap(),
        smelly_connect::resource::ResourceSet::default(),
        smelly_connect::resolver::SessionResolver::new(
            std::collections::HashMap::new(),
            None,
            system_dns,
        ),
        smelly_connect::session::EasyConnectSession::failing_transport("unused"),
    )
    .with_local_route_overrides(smelly_connect::session::LocalRouteOverrides::new(
        [(
            "*.foo.edu.cn".to_string(),
            smelly_connect::resource::DomainRule {
                port_min: 443,
                port_max: 443,
                protocol: smelly_connect::RouteProtocol::Tcp,
            },
        )]
        .into_iter()
        .collect(),
        vec![smelly_connect::resource::IpRule {
            ip_min: "42.62.107.1".parse().unwrap(),
            ip_max: "42.62.107.254".parse().unwrap(),
            port_min: 1,
            port_max: 65535,
            protocol: smelly_connect::RouteProtocol::All,
        }],
    ));

    let domain_route = session
        .plan_tcp_connect(("portal.foo.edu.cn", 443))
        .await
        .unwrap();
    assert!(matches!(
        domain_route,
        smelly_connect::session::RoutePlan::VpnResolved(_)
    ));

    let ip_route = session
        .plan_tcp_connect(("42.62.107.8", 443))
        .await
        .unwrap();
    assert!(matches!(
        ip_route,
        smelly_connect::session::RoutePlan::VpnResolved(_)
    ));
}

#[tokio::test]
async fn tcp_only_domain_rule_does_not_allow_udp_send() {
    let mut resources = smelly_connect::resource::ResourceSet::default();
    resources.domain_rules.insert(
        "portal.foo.edu.cn".to_string(),
        smelly_connect::resource::DomainRule {
            port_min: 1,
            port_max: 65535,
            protocol: smelly_connect::RouteProtocol::Tcp,
        },
    );

    let mut system_dns = std::collections::HashMap::new();
    system_dns.insert(
        "portal.foo.edu.cn".to_string(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
    );

    let transport = smelly_connect::session::EasyConnectSession::failing_transport("unused")
        .with_udp_binder(|| async {
            let socket = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            Ok(smelly_connect::transport::VpnUdpSocket::new(socket))
        });
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

    let socket = session.bind_udp().await.unwrap();
    let err = socket
        .send_to(b"ping", ("portal.foo.edu.cn", 53))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed
        )
    ));
}

#[tokio::test]
async fn udp_only_domain_rule_does_not_allow_tcp_connect() {
    let mut resources = smelly_connect::resource::ResourceSet::default();
    resources.domain_rules.insert(
        "portal.foo.edu.cn".to_string(),
        smelly_connect::resource::DomainRule {
            port_min: 1,
            port_max: 65535,
            protocol: smelly_connect::RouteProtocol::Udp,
        },
    );

    let mut system_dns = std::collections::HashMap::new();
    system_dns.insert(
        "portal.foo.edu.cn".to_string(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
    );

    let session = smelly_connect::session::EasyConnectSession::new(
        "10.0.0.8".parse().unwrap(),
        resources,
        smelly_connect::resolver::SessionResolver::new(
            std::collections::HashMap::new(),
            None,
            system_dns,
        ),
        smelly_connect::session::EasyConnectSession::failing_transport("unused"),
    );

    let err = session
        .plan_tcp_connect(("portal.foo.edu.cn", 443))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed
        )
    ));
}
