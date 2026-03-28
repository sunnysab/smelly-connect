#[tokio::test]
async fn inspect_route_reports_library_allow_decision() {
    let output =
        smelly_connect_cli::commands::inspect::inspect_route_for_test("libdb.zju.edu.cn", 443)
            .await;
    assert!(output.contains("allowed"));
}

#[tokio::test]
async fn inspect_session_reports_pool_summary() {
    let output = smelly_connect_cli::commands::inspect::inspect_session_for_test().await;
    assert!(output.contains("ready="));
}

#[test]
fn inspect_session_returns_typed_error_for_missing_config() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(smelly_connect_cli::commands::inspect::run_session_with_config_typed(
            "/definitely/missing/config.toml",
        ))
        .unwrap_err();
    assert!(matches!(err, smelly_connect_cli::error::CliError::Config(_)));
}
