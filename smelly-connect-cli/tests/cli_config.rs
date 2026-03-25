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
fn explicit_config_path_overrides_default_config_toml_lookup() {
    let merged =
        smelly_connect_cli::config::load_for_test("tests/fixtures/config.sample.toml").unwrap();
    assert_eq!(merged.accounts.len(), 2);
}
