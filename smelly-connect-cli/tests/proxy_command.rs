#![cfg(not(feature = "management-api"))]

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
        allow_all: false,
    };

    let err = smelly_connect_cli::commands::proxy::run_proxy(&path, &command)
        .await
        .unwrap_err();

    assert!(err.contains("management-api"));
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn proxy_command_returns_typed_error_when_management_feature_is_missing() {
    let path = write_temp_config(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60

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
        allow_all: false,
    };

    let err = smelly_connect_cli::commands::proxy::run_proxy_typed(&path, &command)
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        smelly_connect_cli::error::CliError::Command(_)
    ));
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn proxy_command_surfaces_listener_failure_instead_of_hanging() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind occupied listener");
    let occupied_addr = occupied.local_addr().expect("occupied listener addr");

    let path = write_temp_config(&format!(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60

        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"

        [proxy.http]
        enabled = true
        listen = "127.0.0.1:0"

        [proxy.socks5]
        enabled = true
        listen = "{occupied_addr}"

        [management]
        enabled = false
        listen = "127.0.0.1:9090"
        "#
    ));
    let command = smelly_connect_cli::cli::ProxyCommand {
        listen_http: None,
        listen_socks5: None,
        prewarm: None,
        keepalive_host: None,
        allow_all: false,
    };

    let err = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        smelly_connect_cli::commands::proxy::run_proxy(&path, &command),
    )
    .await
    .expect("proxy command should not hang when a listener fails")
    .unwrap_err();

    assert!(err.contains("address") || err.contains("listener"));
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn proxy_command_returns_after_shutdown_signal() {
    let path = write_temp_config(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60

        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"

        [proxy.http]
        enabled = true
        listen = "127.0.0.1:0"

        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"

        [management]
        enabled = false
        listen = "127.0.0.1:9090"
        "#,
    );
    let command = smelly_connect_cli::cli::ProxyCommand {
        listen_http: None,
        listen_socks5: None,
        prewarm: None,
        keepalive_host: None,
        allow_all: false,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let proxy = tokio::spawn({
        let path = path.clone();
        let command = command.clone();
        async move {
            smelly_connect_cli::commands::proxy::run_proxy_typed_with_shutdown(
                &path,
                &command,
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    shutdown_tx.send(()).expect("trigger shutdown");

    let result = tokio::time::timeout(std::time::Duration::from_millis(250), proxy)
        .await
        .expect("proxy command should return after shutdown")
        .expect("proxy task should join");

    assert!(result.is_ok(), "expected graceful shutdown, got {result:?}");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn proxy_command_forces_shutdown_after_drain_timeout() {
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe listener");
    let listen_addr = probe.local_addr().expect("probe listener addr");
    drop(probe);

    let path = write_temp_config(&format!(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60

        [proxy]
        shutdown_drain_timeout_secs = 0

        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"

        [proxy.http]
        enabled = true
        listen = "{listen_addr}"

        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"

        [management]
        enabled = false
        listen = "127.0.0.1:9090"
        "#
    ));
    let command = smelly_connect_cli::cli::ProxyCommand {
        listen_http: None,
        listen_socks5: None,
        prewarm: None,
        keepalive_host: None,
        allow_all: false,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let proxy = tokio::spawn({
        let path = path.clone();
        let command = command.clone();
        async move {
            smelly_connect_cli::commands::proxy::run_proxy_typed_with_shutdown(
                &path,
                &command,
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
        }
    });

    let client = wait_for_listener(listen_addr).await;
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    shutdown_tx.send(()).expect("trigger shutdown");

    let result = tokio::time::timeout(std::time::Duration::from_millis(250), proxy)
        .await
        .expect("proxy command should force shutdown after timeout")
        .expect("proxy task should join");

    assert!(result.is_ok(), "expected forced graceful shutdown, got {result:?}");
    drop(client);
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

async fn wait_for_listener(addr: std::net::SocketAddr) -> tokio::net::TcpStream {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(250);
    loop {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(stream) => return stream,
            Err(err) if tokio::time::Instant::now() < deadline => {
                let _ = err;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(err) => panic!("listener did not start in time: {err}"),
        }
    }
}
