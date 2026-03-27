#![cfg(feature = "management-api")]

use std::collections::BTreeMap;

#[tokio::test]
async fn management_health_endpoint_reports_pool_state() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();
    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/healthz")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["pool"]["total_nodes"], 1);
    assert_eq!(json["pool"]["selectable_nodes"], 1);
    assert!(json["pool"].get("nodes").is_none());
}

#[tokio::test]
async fn management_health_reflects_recent_connect_failures_until_a_success_resets_it() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();
    stats.record_connect_failure_for_test();

    let body = smelly_connect_cli::management::fetch_json_for_test(pool.clone(), stats.clone(), "/healthz")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "recovering");
    assert_eq!(json["pool"]["status"], "recovering");

    stats.record_connect_success_for_test();
    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/healthz")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["pool"]["status"], "healthy");
}

#[tokio::test]
async fn management_health_reports_timed_open_pool_as_recovering() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();

    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/healthz")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "recovering");
    assert_eq!(json["pool"]["status"], "recovering");
    assert_eq!(json["pool"]["open_nodes"], 1);
}

#[tokio::test]
async fn management_health_reports_configured_capacity_as_recovering() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(2, 0).await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();

    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/healthz")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "recovering");
    assert_eq!(json["pool"]["status"], "recovering");
    assert_eq!(json["pool"]["configured_nodes"], 2);
}

#[tokio::test]
async fn management_stats_endpoint_reports_connection_and_traffic_counters() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();
    let mut http = BTreeMap::new();
    http.insert("current_connections", 1_u64);
    http.insert("total_connections", 2_u64);
    http.insert("client_to_upstream_bytes", 30_u64);
    http.insert("upstream_to_client_bytes", 70_u64);
    stats.seed_protocol_for_test("http", http);

    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/stats")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["http"]["current_connections"], 1);
    assert_eq!(json["http"]["total_connections"], 2);
    assert_eq!(json["http"]["client_to_upstream_bytes"], 30);
    assert_eq!(json["http"]["upstream_to_client_bytes"], 70);
    assert!(json["pool"].get("nodes").is_none());
}

#[tokio::test]
async fn management_nodes_endpoint_reports_verbose_node_states() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();
    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/nodes")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["total_nodes"], 1);
    assert_eq!(json["nodes"][0]["name"], "acct-01");
    assert_eq!(json["nodes"][0]["state"], "ready");
}

#[tokio::test]
async fn management_routes_endpoint_reports_route_tables() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_live_accounts([(
        "acct-01",
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    )])
    .await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();
    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/routes")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["total_nodes"], 1);
    assert_eq!(json["nodes"][0]["name"], "acct-01");
    assert_eq!(json["nodes"][0]["state"], "ready");
    assert_eq!(
        json["nodes"][0]["routes"]["domain_rules"][0]["domain"],
        "jwxt.sit.edu.cn"
    );
    assert_eq!(
        json["nodes"][0]["routes"]["ip_rules"][0]["ip_min"],
        "10.0.0.8"
    );
}

#[tokio::test]
async fn management_routes_endpoint_reports_local_overrides_separately() {
    let session = smelly_connect::session::tests::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    )
    .with_local_route_overrides(smelly_connect::session::LocalRouteOverrides::new(
        [(
            "*.foo.edu.cn".to_string(),
            smelly_connect::resource::DomainRule {
                port_min: 443,
                port_max: 443,
                protocol: "tcp".to_string(),
            },
        )]
        .into_iter()
        .collect(),
        vec![smelly_connect::resource::IpRule {
            ip_min: "42.62.107.1".parse().unwrap(),
            ip_max: "42.62.107.254".parse().unwrap(),
            port_min: 1,
            port_max: 65535,
            protocol: "all".to_string(),
        }],
    ));
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_for_test(vec![("acct-01", session)])
            .await;
    let stats = smelly_connect_cli::runtime::RuntimeStats::default();
    let body = smelly_connect_cli::management::fetch_json_for_test(pool, stats, "/routes")
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["nodes"][0]["local_routes"]["domain_rules"][0]["domain"],
        ".foo.edu.cn"
    );
    assert_eq!(
        json["nodes"][0]["local_routes"]["ip_rules"][0]["ip_min"],
        "42.62.107.1"
    );
}
