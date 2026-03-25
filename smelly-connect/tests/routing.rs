#[tokio::test]
async fn routing_rejects_non_resource_targets_by_default() {
    let session = smelly_connect::session::tests::fake_session_without_match();
    let err = session
        .plan_tcp_connect(("example.com", 443))
        .await
        .unwrap_err();
    assert!(matches!(err, smelly_connect::Error::RouteDecision(_)));
}

#[tokio::test]
async fn config_connect_builds_session_with_client_ip() {
    let harness = smelly_connect::session::tests::login_harness();
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
            protocol: "all".to_string(),
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
