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
