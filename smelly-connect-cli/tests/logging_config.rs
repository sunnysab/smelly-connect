#[test]
fn logging_defaults_to_stdout_info_and_default_file() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
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
    assert_eq!(cfg.logging.mode.as_str(), "stdout");
    assert_eq!(cfg.logging.level.as_str(), "info");
    assert_eq!(cfg.logging.file, "smelly-connect.log");
}

#[test]
fn terminal_logging_mode_means_stderr_not_stdout() {
    let cfg: smelly_connect_cli::config::AppConfig =
        toml::from_str(include_str!("fixtures/config.logging.stdout.toml")).unwrap();
    assert_eq!(cfg.logging.mode.as_str(), "stdout");
}

#[test]
fn logging_level_is_parsed_and_available_for_filtering() {
    let cfg: smelly_connect_cli::config::AppConfig =
        toml::from_str(include_str!("fixtures/config.logging.file.toml")).unwrap();
    assert_eq!(cfg.logging.level.as_str(), "warn");
}

#[test]
fn invalid_logging_mode_is_rejected() {
    let cfg = r#"
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
        [logging]
        mode = "bogus"
    "#;
    assert!(toml::from_str::<smelly_connect_cli::config::AppConfig>(cfg).is_err());
}

#[test]
fn invalid_logging_level_is_rejected() {
    let cfg = r#"
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
        [logging]
        level = "bogus"
    "#;
    assert!(toml::from_str::<smelly_connect_cli::config::AppConfig>(cfg).is_err());
}
