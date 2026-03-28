#[tokio::test]
async fn proxy_forwards_http_requests_through_session() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness().await;
    let body = harness
        .get_via_proxy("http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn proxy_supports_https_connect() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness().await;
    harness
        .connect_tunnel("libdb.zju.edu.cn:443")
        .await
        .unwrap();
}

#[tokio::test]
async fn proxy_streams_split_http_request_body() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness_with_body_echo().await;
    let body = harness
        .post_split_body_via_proxy("http://intranet.zju.edu.cn/upload", "hello", " world")
        .await;
    assert_eq!(body, "hello world");
}

#[tokio::test]
async fn proxy_completes_body_when_upstream_keeps_connection_alive() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness_with_keep_alive().await;
    let body = harness
        .get_via_proxy_with_connection("http://intranet.zju.edu.cn/index.html", "keep-alive")
        .await;
    assert_eq!(body, "hello");
}

#[tokio::test]
async fn proxy_streams_split_chunked_http_request_body() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness_with_chunked_body_echo().await;
    let body = harness
        .post_split_chunked_body_via_proxy(
            "http://intranet.zju.edu.cn/upload",
            "5\r\nhello\r\n",
            "6\r\n world\r\n0\r\n\r\n",
        )
        .await;
    assert_eq!(body, "hello world");
}

#[tokio::test]
async fn proxy_handles_expect_100_continue_requests() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness_with_body_echo().await;
    let body = harness
        .post_expect_continue_via_proxy("http://intranet.zju.edu.cn/upload", "hello world")
        .await;
    assert_eq!(body, "hello world");
}

#[tokio::test]
async fn proxy_does_not_forward_proxy_authorization_header() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness_with_proxy_auth_capture().await;
    let body = harness
        .get_with_proxy_authorization_via_proxy(
            "http://intranet.zju.edu.cn/health",
            "Basic Zm9vOmJhcg==",
        )
        .await;
    assert_eq!(body, "clean");
}

#[tokio::test]
async fn proxy_rejects_oversized_header_block() {
    let harness = smelly_connect::test_support::proxy::http_proxy_harness().await;
    let status = harness.oversized_header_status_via_proxy().await;
    assert_eq!(status, 431);
}
