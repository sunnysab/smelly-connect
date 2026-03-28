#[tokio::test]
async fn status_command_reports_health_and_runtime_stats() {
    let output = smelly_connect_cli::commands::status::run_status_for_test(
        "127.0.0.1:19090",
        r#"{
            "status":"healthy",
            "pool":{
                "status":"healthy",
                "total_nodes":2,
                "selectable_nodes":2,
                "ready_nodes":2,
                "suspect_nodes":0,
                "open_nodes":0,
                "half_open_nodes":0,
                "connecting_nodes":0,
                "configured_nodes":0
            }
        }"#,
        r#"{
            "status":"healthy",
            "pool":{
                "status":"healthy",
                "total_nodes":2,
                "selectable_nodes":2,
                "ready_nodes":2,
                "suspect_nodes":0,
                "open_nodes":0,
                "half_open_nodes":0,
                "connecting_nodes":0,
                "configured_nodes":0
            },
            "total":{
                "current_connections":3,
                "total_connections":9,
                "client_to_upstream_bytes":120,
                "upstream_to_client_bytes":240
            },
            "http":{
                "current_connections":1,
                "total_connections":4,
                "client_to_upstream_bytes":40,
                "upstream_to_client_bytes":90
            },
            "socks5":{
                "current_connections":2,
                "total_connections":5,
                "client_to_upstream_bytes":80,
                "upstream_to_client_bytes":150
            }
        }"#,
    )
    .await
    .unwrap();

    assert!(output.contains("management=127.0.0.1:19090"));
    assert!(output.contains("status=healthy"));
    assert!(output.contains("pool total=2 selectable=2 ready=2"));
    assert!(output.contains("total current=3 total=9 c2u=120 u2c=240"));
    assert!(output.contains("http current=1 total=4 c2u=40 u2c=90"));
    assert!(output.contains("socks5 current=2 total=5 c2u=80 u2c=150"));
}

#[tokio::test]
async fn http_connect_failure_marks_runtime_status_recovering() {
    let snapshot =
        smelly_connect_cli::proxy::http::proxy_http_connect_failure_runtime_status_for_test()
            .await
            .unwrap();
    assert_eq!(snapshot.status, smelly_connect_cli::pool::PoolHealthStatus::Recovering);
}

#[test]
fn status_command_returns_typed_error_when_management_is_disabled() {
    let path = std::env::temp_dir().join("smelly-connect-cli-status-no-management.toml");
    std::fs::write(
        &path,
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60

        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"

        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"

        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        "#,
    )
    .unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(smelly_connect_cli::commands::status::run_status_with_config_typed(
            &path,
        ))
        .unwrap_err();
    assert!(matches!(err, smelly_connect_cli::error::CliError::Command(_)));
    let _ = std::fs::remove_file(path);
}
