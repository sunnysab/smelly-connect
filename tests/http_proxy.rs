#[tokio::test]
async fn proxy_forwards_http_requests_through_session() {
    let harness = smelly_connect::proxy::tests::http_proxy_harness().await;
    let body = harness
        .get_via_proxy("http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn proxy_supports_https_connect() {
    let harness = smelly_connect::proxy::tests::http_proxy_harness().await;
    harness.connect_tunnel("libdb.zju.edu.cn:443").await.unwrap();
}
