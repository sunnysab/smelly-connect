use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn proxy_command_rejects_management_config_when_feature_is_disabled() {
    let path = write_temp_config(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"

        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"

        [proxy.http]
        enabled = false
        listen = "127.0.0.1:8080"

        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"

        [management]
        enabled = true
        listen = "127.0.0.1:9090"
        "#,
    );
    let command = smelly_connect_cli::cli::ProxyCommand {
        listen_http: None,
        listen_socks5: None,
        prewarm: None,
        keepalive_host: None,
    };

    let err = smelly_connect_cli::commands::proxy::run_proxy(&path, &command)
        .await
        .unwrap_err();

    assert!(err.contains("management-api"));
    let _ = fs::remove_file(path);
}

fn write_temp_config(body: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("smelly-connect-cli-proxy-test-{unique}.toml"));
    fs::write(&path, body).expect("write temp config");
    path
}
