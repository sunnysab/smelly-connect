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

#[test]
fn test_tcp_returns_typed_error_for_missing_port() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(smelly_connect_cli::commands::test::run_tcp_with_config_typed(
            "tests/fixtures/config.sample.toml",
            "10.0.0.8",
        ))
        .unwrap_err();
    assert!(matches!(err, smelly_connect_cli::error::CliError::Command(_)));
}

#[test]
fn test_icmp_returns_typed_error_for_missing_config() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(smelly_connect_cli::commands::test::run_icmp_with_config_typed(
            "/definitely/missing/config.toml",
            "10.0.0.8",
        ))
        .unwrap_err();
    assert!(matches!(err, smelly_connect_cli::error::CliError::Config(_)));
}

#[test]
fn test_http_returns_typed_error_for_missing_config() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(smelly_connect_cli::commands::test::run_http_with_config_typed(
            "/definitely/missing/config.toml",
            "http://intranet.zju.edu.cn/health",
        ))
        .unwrap_err();
    assert!(matches!(err, smelly_connect_cli::error::CliError::Config(_)));
}
