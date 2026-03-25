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
