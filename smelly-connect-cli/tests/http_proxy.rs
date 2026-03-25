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
