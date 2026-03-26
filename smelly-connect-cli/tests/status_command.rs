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
