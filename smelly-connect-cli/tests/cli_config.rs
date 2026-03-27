#[test]
fn defaults_to_config_toml_in_cwd() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(["smelly-connect-cli", "proxy"]);
    assert_eq!(cli.config_path().to_string_lossy(), "config.toml");
}

#[test]
fn parses_sample_config() {
    let cfg: smelly_connect_cli::config::AppConfig =
        toml::from_str(include_str!("fixtures/config.sample.toml")).unwrap();
    assert_eq!(cfg.accounts.len(), 2);
    assert_eq!(cfg.pool.prewarm, 2);
    assert!(!cfg.management.enabled);
    assert_eq!(cfg.management.listen, "127.0.0.1:9090");
}

#[test]
fn parses_local_routing_overrides_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"

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

        [[routing.domain_rules]]
        domain = "*.foo.edu.cn"
        port_min = 443
        port_max = 443
        protocol = "tcp"

        [[routing.ip_rules]]
        ip_min = "42.62.107.1"
        ip_max = "42.62.107.254"
        port_min = 1
        port_max = 65535
        protocol = "all"
        "#,
    )
    .unwrap();

    assert_eq!(cfg.routing.domain_rules.len(), 1);
    assert_eq!(cfg.routing.domain_rules[0].domain, "*.foo.edu.cn");
    assert_eq!(cfg.routing.ip_rules.len(), 1);
    assert_eq!(cfg.routing.ip_rules[0].ip_min, "42.62.107.1");
    assert_eq!(cfg.routing.ip_rules[0].ip_max.as_deref(), Some("42.62.107.254"));
}

#[test]
fn parses_allow_all_routing_flag_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"

        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"

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

        [routing]
        allow_all = true
        "#,
    )
    .unwrap();

    assert!(cfg.routing.allow_all);
}

#[test]
fn proxy_command_accepts_config_and_listener_overrides() {
    let cli = smelly_connect_cli::cli::Cli::parse_from([
        "smelly-connect-cli",
        "--config",
        "config.toml",
        "proxy",
        "--listen-http",
        "127.0.0.1:8080",
        "--listen-socks5",
        "127.0.0.1:1080",
    ]);
    assert!(matches!(
        cli.command,
        smelly_connect_cli::cli::Command::Proxy(_)
    ));
}

#[test]
fn status_is_available_as_a_top_level_command() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(["smelly-connect-cli", "status"]);
    assert!(matches!(
        cli.command,
        smelly_connect_cli::cli::Command::Status
    ));
}

#[test]
fn routes_is_available_as_a_top_level_command() {
    let cli = smelly_connect_cli::cli::Cli::parse_from(["smelly-connect-cli", "routes"]);
    assert!(matches!(
        cli.command,
        smelly_connect_cli::cli::Command::Routes
    ));
}

#[test]
fn cli_flags_override_config_values() {
    let merged = smelly_connect_cli::config::merge_for_test(
        "tests/fixtures/config.sample.toml",
        ["--prewarm", "5", "--listen-http", "127.0.0.1:18080"],
    )
    .unwrap();
    assert_eq!(merged.pool.prewarm, 5);
    assert_eq!(merged.proxy.http.listen, "127.0.0.1:18080");
}

#[test]
fn allow_all_flag_overrides_config_values() {
    let merged = smelly_connect_cli::config::merge_for_test(
        "tests/fixtures/config.sample.toml",
        ["--allow-all"],
    )
    .unwrap();
    assert!(merged.routing.allow_all);
}

#[test]
fn explicit_config_path_overrides_default_config_toml_lookup() {
    let merged =
        smelly_connect_cli::config::load_for_test("tests/fixtures/config.sample.toml").unwrap();
    assert_eq!(merged.accounts.len(), 2);
}
