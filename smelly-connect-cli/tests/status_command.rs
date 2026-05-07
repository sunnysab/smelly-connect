#![cfg(feature = "test-utils")]

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
                "disabled_auth_nodes":0,
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
                "disabled_auth_nodes":0,
                "half_open_nodes":0,
                "connecting_nodes":0,
                "configured_nodes":0
            },
            "total":{
                "current_connections":3,
                "total_connections":9,
                "client_to_upstream_bytes":120,
                "upstream_to_client_bytes":240,
                "service_unavailable_no_ready_session": 4,
                "service_unavailable_over_capacity": 1
            },
            "http":{
                "current_connections":1,
                "total_connections":4,
                "client_to_upstream_bytes":40,
                "upstream_to_client_bytes":90,
                "service_unavailable_no_ready_session": 3,
                "service_unavailable_over_capacity": 1
            },
            "socks5":{
                "current_connections":2,
                "total_connections":5,
                "client_to_upstream_bytes":80,
                "upstream_to_client_bytes":150,
                "service_unavailable_no_ready_session": 1,
                "service_unavailable_over_capacity": 0
            }
        }"#,
    )
    .await
    .unwrap();

    assert!(output.contains("management=127.0.0.1:19090"));
    assert!(output.contains("status=healthy"));
    assert!(output.contains("pool total=2 selectable=2 ready=2 suspect=0 open=0 disabled_auth=0"));
    assert!(output.contains(
        "total current=3 total=9 c2u=120 B u2c=240 B svc503_no_ready=4 svc503_over_capacity=1"
    ));
    assert!(output.contains(
        "http current=1 total=4 c2u=40 B u2c=90 B svc503_no_ready=3 svc503_over_capacity=1"
    ));
    assert!(output.contains(
        "socks5 current=2 total=5 c2u=80 B u2c=150 B svc503_no_ready=1 svc503_over_capacity=0"
    ));
}

#[tokio::test]
async fn status_command_prefers_management_api_override_over_config() {
    let path = std::env::temp_dir().join("smelly-connect-cli-status-management-override.toml");
    std::fs::write(
        &path,
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        min_pool_size = 1
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

        [management]
        enabled = true
        listen = "127.0.0.1:1"
        "#,
    )
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let health_body = r#"{
        "status":"healthy",
        "pool":{
            "total_nodes":1,
            "selectable_nodes":1,
            "ready_nodes":1,
            "suspect_nodes":0,
            "open_nodes":0,
            "half_open_nodes":0,
            "connecting_nodes":0,
            "configured_nodes":1
        }
    }"#
    .to_string();
    let stats_body = r#"{
        "total":{
            "current_connections":1,
            "total_connections":2,
            "client_to_upstream_bytes":3,
            "upstream_to_client_bytes":4
        },
        "http":{
            "current_connections":0,
            "total_connections":1,
            "client_to_upstream_bytes":1,
            "upstream_to_client_bytes":2
        },
        "socks5":{
            "current_connections":1,
            "total_connections":1,
            "client_to_upstream_bytes":2,
            "upstream_to_client_bytes":2
        }
    }"#
    .to_string();
    tokio::spawn(async move {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = vec![0_u8; 1024];
            let Ok(n) = tokio::io::AsyncReadExt::read(&mut stream, &mut request).await else {
                return;
            };
            let request = String::from_utf8_lossy(&request[..n]);
            let body = if request.starts_with("GET /healthz ") {
                &health_body
            } else {
                &stats_body
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        }
    });

    let output = smelly_connect_cli::commands::status::run_status_with_config_and_management_api(
        &path,
        Some(addr.to_string()),
    )
    .await
    .unwrap();

    assert!(output.contains(&format!("management={addr}")));
    assert!(output.contains("status=healthy"));
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn http_connect_failure_marks_runtime_status_recovering() {
    let snapshot =
        smelly_connect_cli::proxy::http::proxy_http_connect_failure_runtime_status_for_test()
            .await
            .unwrap();
    assert_eq!(
        snapshot.status,
        smelly_connect_cli::pool::PoolHealthStatus::Recovering
    );
}

#[tokio::test]
async fn status_command_formats_large_byte_counters_with_human_units() {
    let output = smelly_connect_cli::commands::status::run_status_for_test(
        "127.0.0.1:19090",
        r#"{
            "status":"healthy",
            "pool":{
                "status":"healthy",
                "total_nodes":1,
                "selectable_nodes":1,
                "ready_nodes":1,
                "suspect_nodes":0,
                "open_nodes":0,
                "half_open_nodes":0,
                "connecting_nodes":0,
                "configured_nodes":1
            }
        }"#,
        r#"{
            "total":{
                "current_connections":1,
                "total_connections":2,
                "client_to_upstream_bytes":1536,
                "upstream_to_client_bytes":2500000
            },
            "http":{
                "current_connections":1,
                "total_connections":2,
                "client_to_upstream_bytes":1000,
                "upstream_to_client_bytes":1000000
            },
            "socks5":{
                "current_connections":0,
                "total_connections":0,
                "client_to_upstream_bytes":0,
                "upstream_to_client_bytes":999
            }
        }"#,
    )
    .await
    .unwrap();

    assert!(output.contains("total current=1 total=2 c2u=1.5 kB u2c=2.5 MB"));
    assert!(output.contains("http current=1 total=2 c2u=1.0 kB u2c=1.0 MB"));
    assert!(output.contains("socks5 current=0 total=0 c2u=0 B u2c=999 B"));
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
        min_pool_size = 1
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
        .block_on(smelly_connect_cli::commands::status::run_status_with_config_typed(&path))
        .unwrap_err();
    assert!(matches!(
        err,
        smelly_connect_cli::error::CliError::Command(_)
    ));
    let _ = std::fs::remove_file(path);
}
