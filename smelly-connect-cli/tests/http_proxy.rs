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
