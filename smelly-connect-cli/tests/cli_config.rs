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
