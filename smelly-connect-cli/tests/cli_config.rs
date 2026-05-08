use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ROOT_CERTIFICATE_PEM: &str = concat!(
    "-----BEGIN CERTIFICATE-----\n",
    "MIIDLTCCAhWgAwIBAgIUZlZmjovNQHxGIfSIwlloLjclr3cwDQYJKoZIhvcNAQEL\n",
    "BQAwHjEcMBoGA1UEAwwTU21lbGx5IFRlc3QgUm9vdCBDQTAeFw0yNjA1MDgwNTU2\n",
    "MTVaFw0zNjA1MDUwNTU2MTVaMB4xHDAaBgNVBAMME1NtZWxseSBUZXN0IFJvb3Qg\n",
    "Q0EwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCpW6sBPwlfP89XYzqp\n",
    "d1K3a/LCR3CXqNhbYrambPwMWwS7Ycct84T/AdkiMtQ+F53VGaO5tU33pxrMxHq0\n",
    "aSAdy5KTitFUaTce9sABBnf8DK5WJTmtSj5cwnjLqffiN1/uK2d97W+9QS7qSlwU\n",
    "E1j97JoU6uoQBmOXPmiByOAqmPeZEF0u7rELAIrRS1Lo6htpGOh3zNu6j7Qmu2mH\n",
    "ScgeSMIjKGr9Yx5KrcyrhkUtgZwRg5C02aNAGiDDpzK/adXwUuUGrWyZoRPHP3IQ\n",
    "PcQ1QDTJf5o0AIETICI8COp1Sa8QLSm2spPC+ApH5vvIScnWHtblbzsLq7LIKPGY\n",
    "41/XAgMBAAGjYzBhMB0GA1UdDgQWBBRZQ+ja7ZwlVmLOJmaDlyGVkBISTTAfBgNV\n",
    "HSMEGDAWgBRZQ+ja7ZwlVmLOJmaDlyGVkBISTTAPBgNVHRMBAf8EBTADAQH/MA4G\n",
    "A1UdDwEB/wQEAwIBBjANBgkqhkiG9w0BAQsFAAOCAQEAnBenddNNgc804TsAUrro\n",
    "CHNrNvnH9BJ2evf1mI80KEUHntkpt4Lgi+YXTSfSxVpQ0Maho/7k2wrl5g+BHINy\n",
    "0nLDaEgnreEcQ2YsHTlKiCXoE87atsHEtsZKiQIpszHkXRgvVjwyracnZHHuiIF8\n",
    "OoJCKtWwzcTr6twbbgFvxZRrE5wTcodn4sqoVG2qWkKCZTVWUuF8F/N4csBpg+MO\n",
    "9Ey/RSKYMf+gI8a/oexydSTefQx9aJFOqOYFoR5FmshV5lJuf6T4C45R/SHXC9K7\n",
    "FcDjMBrd4LsN4jnpXJnYu9q+QplajcmyI2kvHnlYaxB2qgmeF1zwT0Ln0SfQGiCR\n",
    "6w==\n",
    "-----END CERTIFICATE-----\n",
);

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn write(path_suffix: &str, contents: impl AsRef<[u8]>) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "smelly-connect-cli-{path_suffix}-{}-{unique}",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn root_certificate_der() -> Vec<u8> {
    rustls_pemfile::certs(&mut Cursor::new(ROOT_CERTIFICATE_PEM.as_bytes()))
        .next()
        .expect("root PEM should contain one certificate")
        .expect("root PEM should parse")
        .to_vec()
}

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
    assert_eq!(cfg.pool.min_pool_size, 2);
    assert!(!cfg.vpn.insecure_skip_verify);
    assert!(cfg.vpn.enable_icmp_keepalive);
    assert_eq!(
        cfg.session_connect_timeout(),
        std::time::Duration::from_secs(7)
    );
    assert_eq!(
        cfg.upstream_tcp_connect_timeout(),
        std::time::Duration::from_secs(3)
    );
    assert_eq!(
        cfg.shutdown_drain_timeout(),
        std::time::Duration::from_secs(12)
    );
    assert_eq!(
        cfg.udp_associate_idle_timeout(),
        Some(std::time::Duration::from_secs(90))
    );
    assert!(!cfg.management.enabled);
    assert_eq!(cfg.management.listen, "127.0.0.1:9090");
    assert_eq!(
        cfg.routing.default_action,
        smelly_connect_cli::config::RoutingDefaultAction::Direct
    );
}

#[test]
fn parses_explicit_insecure_skip_verify_flag() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        insecure_skip_verify = true

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

    assert!(cfg.vpn.insecure_skip_verify);
    assert_eq!(
        cfg.server_cert_policy().unwrap(),
        smelly_connect::ServerCertPolicy::InsecureSkipVerify
    );
}

#[test]
fn loads_custom_root_certificate_from_der_file() {
    let der = TempFile::write("root.der", root_certificate_der());
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(&format!(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        ca_cert = "{}"

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
        der.path().display()
    ))
    .unwrap();

    assert_eq!(
        cfg.server_cert_policy().unwrap(),
        smelly_connect::ServerCertPolicy::VerifyWithCustomRoots(vec![root_certificate_der()])
    );
}

#[test]
fn loads_custom_root_certificates_from_pem_bundle() {
    let pem = TempFile::write(
        "root.pem",
        format!("{ROOT_CERTIFICATE_PEM}{ROOT_CERTIFICATE_PEM}"),
    );
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(&format!(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        ca_cert = "{}"

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
        pem.path().display()
    ))
    .unwrap();

    assert_eq!(
        cfg.server_cert_policy().unwrap(),
        smelly_connect::ServerCertPolicy::VerifyWithCustomRoots(vec![
            root_certificate_der(),
            root_certificate_der(),
        ])
    );
}

#[test]
fn insecure_skip_verify_takes_precedence_over_custom_root_file() {
    let missing_path = std::env::temp_dir().join(format!(
        "smelly-connect-cli-missing-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(&format!(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        insecure_skip_verify = true
        ca_cert = "{}"

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
        missing_path.display()
    ))
    .unwrap();

    assert_eq!(
        cfg.server_cert_policy().unwrap(),
        smelly_connect::ServerCertPolicy::InsecureSkipVerify
    );
}

#[test]
fn routing_default_action_defaults_to_direct() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
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

    assert_eq!(
        cfg.routing.default_action,
        smelly_connect_cli::config::RoutingDefaultAction::Direct
    );
}

#[test]
fn parses_block_default_action_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
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

        [routing]
        default_action = "block"
        "#,
    )
    .unwrap();

    assert_eq!(
        cfg.routing.default_action,
        smelly_connect_cli::config::RoutingDefaultAction::Block
    );
}

#[test]
fn legacy_connect_timeout_still_applies_when_split_fields_are_absent() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
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

    assert_eq!(
        cfg.session_connect_timeout(),
        std::time::Duration::from_secs(20)
    );
    assert_eq!(
        cfg.upstream_tcp_connect_timeout(),
        std::time::Duration::from_secs(20)
    );
    assert_eq!(
        cfg.shutdown_drain_timeout(),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(cfg.udp_associate_idle_timeout(), None);
}

#[test]
fn parses_local_routing_overrides_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
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
    assert_eq!(
        cfg.routing.ip_rules[0].ip_max.as_deref(),
        Some("42.62.107.254")
    );
}

#[test]
fn parses_allow_all_routing_flag_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
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
        smelly_connect_cli::cli::Command::Status(_)
    ));
}

#[test]
fn status_accepts_management_api_override() {
    let cli = smelly_connect_cli::cli::Cli::parse_from([
        "smelly-connect-cli",
        "status",
        "--management-api",
        "127.0.0.1:19090",
    ]);
    assert!(matches!(
        cli.command,
        smelly_connect_cli::cli::Command::Status(smelly_connect_cli::cli::StatusCommand {
            management_api: Some(ref management_api),
        }) if management_api == "127.0.0.1:19090"
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
        ["--min-pool-size", "5", "--listen-http", "127.0.0.1:18080"],
    )
    .unwrap();
    assert_eq!(merged.pool.min_pool_size, 5);
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

#[test]
fn invalid_route_protocol_is_rejected() {
    let cfg = toml::from_str::<smelly_connect_cli::config::AppConfig>(
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

        [[routing.domain_rules]]
        domain = "*.foo.edu.cn"
        port_min = 443
        port_max = 443
        protocol = "icmp"
        "#,
    );

    assert!(cfg.is_err(), "invalid route protocol should be rejected");
}

#[test]
fn invalid_routing_default_action_is_rejected() {
    let cfg = toml::from_str::<smelly_connect_cli::config::AppConfig>(
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

        [routing]
        default_action = "drop"
        "#,
    );

    assert!(cfg.is_err(), "invalid default action should be rejected");
}

#[test]
fn missing_config_returns_typed_cli_error() {
    let err =
        smelly_connect_cli::config::load_typed("/definitely/missing/config.toml").unwrap_err();
    assert!(matches!(
        err,
        smelly_connect_cli::error::CliError::Config(_)
    ));
}

#[test]
fn parses_explicit_icmp_disable_flag_from_config() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        default_keepalive_host = "jwxt.sit.edu.cn"
        enable_icmp_keepalive = false

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

    assert!(!cfg.vpn.enable_icmp_keepalive);
}
