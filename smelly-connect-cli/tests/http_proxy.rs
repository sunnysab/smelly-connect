#[tokio::test]
async fn http_proxy_uses_pool_and_forwards_requests() {
    let result = smelly_connect_cli::proxy::http::proxy_http_for_test()
        .await
        .unwrap();
    assert_eq!(result.body, "ok");
    assert_eq!(result.account_name, "acct-01");
    assert!(result.used_pool_selection);
}

#[tokio::test]
async fn http_connect_proxy_tunnels_bytes_through_selected_session() {
    let result = smelly_connect_cli::proxy::http::proxy_connect_for_test()
        .await
        .unwrap();
    assert_eq!(result.account_name, "acct-01");
    assert_eq!(result.echoed_bytes, b"ping");
}

#[tokio::test]
async fn http_proxy_fails_fast_when_pool_has_no_ready_session() {
    let result = smelly_connect_cli::proxy::http::proxy_http_no_ready_session_for_test()
        .await
        .unwrap();
    assert_eq!(result.status_code, 503);
}

#[tokio::test]
async fn http_proxy_listener_stays_bound_during_total_pool_outage() {
    let results = smelly_connect_cli::proxy::http::proxy_http_no_ready_session_sequence_for_test(2)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.status_code == 503));
}

#[tokio::test]
async fn http_proxy_updates_runtime_stats_after_forwarding() {
    let snapshot = smelly_connect_cli::proxy::http::proxy_http_runtime_stats_for_test()
        .await
        .unwrap();
    assert_eq!(snapshot.http.current_connections, 0);
    assert_eq!(snapshot.http.total_connections, 1);
    assert!(snapshot.http.client_to_upstream_bytes > 0);
    assert!(snapshot.http.upstream_to_client_bytes > 0);
}

#[tokio::test]
async fn http_proxy_enforces_connect_timeout() {
    let result = smelly_connect_cli::proxy::http::proxy_http_connect_timeout_for_test()
        .await
        .unwrap();
    assert!(result.elapsed < std::time::Duration::from_millis(150));
}

#[tokio::test]
async fn http_connect_returns_bad_gateway_on_upstream_connect_failure() {
    let result = smelly_connect_cli::proxy::http::proxy_connect_failure_status_for_test()
        .await
        .unwrap();
    assert_eq!(result.status_code, 502);
}

#[tokio::test]
async fn http_connect_returns_gateway_timeout_on_upstream_timeout() {
    let result = smelly_connect_cli::proxy::http::proxy_connect_timeout_status_for_test()
        .await
        .unwrap();
    assert_eq!(result.status_code, 504);
}

#[tokio::test(start_paused = true)]
async fn http_live_connect_failure_marks_node_open_for_recovery() {
    let result = smelly_connect_cli::proxy::http::proxy_http_live_connect_failure_recovery_for_test()
        .await
        .unwrap();
    assert_eq!(result.status_code, 502);
    assert!(result.state_summary.contains("Open"));
    assert!(!result.selectable_after_failure);
    assert_eq!(result.recovered_account, "acct-01");
}

#[tokio::test]
async fn http_route_rejection_does_not_mark_live_session_open() {
    let result = smelly_connect_cli::proxy::http::proxy_http_route_rejection_does_not_open_for_test()
        .await
        .unwrap();
    assert_eq!(result.status_code, 502);
    assert!(result.state_summary.contains("Ready"));
    assert!(result.selectable_after_failure);
}
