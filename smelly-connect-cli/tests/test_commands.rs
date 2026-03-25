#[tokio::test]
async fn test_tcp_reports_success_on_connect() {
    let output = smelly_connect_cli::commands::test::run_tcp_for_test("10.0.0.8:443")
        .await
        .unwrap();
    assert!(output.contains("tcp ok"));
}

#[tokio::test]
async fn test_icmp_uses_session_level_ping() {
    let output = smelly_connect_cli::commands::test::run_icmp_for_test("10.0.0.8")
        .await
        .unwrap();
    assert!(output.contains("icmp ok"));
}

#[tokio::test]
async fn test_http_fetches_url() {
    let output =
        smelly_connect_cli::commands::test::run_http_for_test("http://intranet.zju.edu.cn/health")
            .await
            .unwrap();
    assert!(output.contains("status="));
}
