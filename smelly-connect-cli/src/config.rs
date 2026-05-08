use serde::Deserialize;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use x509_cert::Certificate;
use x509_cert::der::Decode;

use crate::cli::ProxyCommand;
use crate::error::CliError;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub vpn: VpnConfig,
    pub pool: PoolConfig,
    pub accounts: Vec<AccountConfig>,
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub management: ManagementConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VpnConfig {
    pub server: String,
    #[serde(default = "default_enable_icmp_keepalive")]
    pub enable_icmp_keepalive: bool,
    pub default_keepalive_host: Option<String>,
    #[serde(default)]
    pub ca_cert: Option<String>,
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PoolConfig {
    pub min_pool_size: usize,
    pub connect_timeout_secs: u64,
    pub session_connect_timeout_secs: Option<u64>,
    pub healthcheck_interval_secs: u64,
    pub failure_threshold: u32,
    pub backoff_base_secs: u64,
    pub backoff_max_secs: u64,
    pub allow_request_triggered_probe: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_pool_size: 1,
            connect_timeout_secs: 20,
            session_connect_timeout_secs: None,
            healthcheck_interval_secs: 60,
            failure_threshold: 3,
            backoff_base_secs: 30,
            backoff_max_secs: 600,
            allow_request_triggered_probe: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub upstream_tcp_connect_timeout_secs: Option<u64>,
    #[serde(default)]
    pub shutdown_drain_timeout_secs: Option<u64>,
    pub http: ListenerConfig,
    pub socks5: Socks5Config,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerConfig {
    pub enabled: bool,
    pub listen: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Socks5Config {
    pub enabled: bool,
    pub listen: String,
    #[serde(default)]
    pub udp_associate_idle_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoutingConfig {
    pub allow_all: bool,
    pub default_action: RoutingDefaultAction,
    pub domain_rules: Vec<LocalDomainRuleConfig>,
    pub ip_rules: Vec<LocalIpRuleConfig>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoutingDefaultAction {
    #[default]
    Direct,
    Block,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalDomainRuleConfig {
    pub domain: String,
    #[serde(default = "default_port_min")]
    pub port_min: u16,
    #[serde(default = "default_port_max")]
    pub port_max: u16,
    #[serde(
        default = "default_protocol",
        deserialize_with = "deserialize_route_protocol"
    )]
    pub protocol: smelly_connect::RouteProtocol,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalIpRuleConfig {
    pub ip_min: String,
    pub ip_max: Option<String>,
    #[serde(default = "default_port_min")]
    pub port_min: u16,
    #[serde(default = "default_port_max")]
    pub port_max: u16,
    #[serde(
        default = "default_protocol",
        deserialize_with = "deserialize_route_protocol"
    )]
    pub protocol: smelly_connect::RouteProtocol,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ManagementConfig {
    pub enabled: bool,
    pub listen: String,
}

impl Default for ManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:9090".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub mode: LoggingMode,
    pub level: LoggingLevel,
    pub file: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            mode: LoggingMode::Stdout,
            level: LoggingLevel::Info,
            file: "smelly-connect.log".to_string(),
        }
    }
}

fn default_port_min() -> u16 {
    1
}

fn default_port_max() -> u16 {
    65535
}

fn default_protocol() -> smelly_connect::RouteProtocol {
    smelly_connect::RouteProtocol::All
}

fn deserialize_route_protocol<'de, D>(
    deserializer: D,
) -> Result<smelly_connect::RouteProtocol, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    smelly_connect::RouteProtocol::from_str(&value).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoggingMode {
    #[serde(rename = "stdout")]
    #[default]
    Stdout,
    #[serde(rename = "file")]
    File,
    #[serde(rename = "stdout+file")]
    StdoutAndFile,
    #[serde(rename = "off")]
    Off,
}

impl LoggingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::File => "file",
            Self::StdoutAndFile => "stdout+file",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoggingLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl LoggingLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

impl AppConfig {
    pub fn server_cert_policy(&self) -> Result<smelly_connect::ServerCertPolicy, CliError> {
        if self.vpn.insecure_skip_verify {
            return Ok(smelly_connect::ServerCertPolicy::InsecureSkipVerify);
        }

        match self.vpn.ca_cert.as_deref() {
            Some(path) => load_custom_root_certificates(Path::new(path))
                .map(smelly_connect::ServerCertPolicy::VerifyWithCustomRoots),
            None => Ok(smelly_connect::ServerCertPolicy::Verify),
        }
    }

    pub fn icmp_keepalive_target(&self) -> Option<&str> {
        if !self.vpn.enable_icmp_keepalive {
            return None;
        }
        self.vpn.default_keepalive_host.as_deref()
    }

    pub fn session_connect_timeout(&self) -> Duration {
        Duration::from_secs(
            self.pool
                .session_connect_timeout_secs
                .unwrap_or(self.pool.connect_timeout_secs)
                .max(1),
        )
    }

    pub fn upstream_tcp_connect_timeout(&self) -> Duration {
        Duration::from_secs(
            self.proxy
                .upstream_tcp_connect_timeout_secs
                .unwrap_or(self.pool.connect_timeout_secs)
                .max(1),
        )
    }

    pub fn shutdown_drain_timeout(&self) -> Duration {
        Duration::from_secs(self.proxy.shutdown_drain_timeout_secs.unwrap_or(30))
    }

    pub fn udp_associate_idle_timeout(&self) -> Option<Duration> {
        self.proxy
            .socks5
            .udp_associate_idle_timeout_secs
            .filter(|secs| *secs > 0)
            .map(Duration::from_secs)
    }
}

fn default_enable_icmp_keepalive() -> bool {
    true
}

fn load_custom_root_certificates(path: &Path) -> Result<Vec<Vec<u8>>, CliError> {
    let pem_or_der = fs::read(path).map_err(|err| {
        CliError::Config(format!(
            "failed to read vpn.ca_cert {}: {err}",
            path.display()
        ))
    })?;

    if looks_like_pem_certificate_bundle(&pem_or_der) {
        let certs = rustls_pemfile::certs(&mut Cursor::new(&pem_or_der))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                CliError::Config(format!(
                    "failed to parse PEM certificates from vpn.ca_cert {}: {err}",
                    path.display()
                ))
            })?;
        if certs.is_empty() {
            return Err(CliError::Config(format!(
                "vpn.ca_cert {} did not contain any PEM certificates",
                path.display()
            )));
        }
        return Ok(certs.into_iter().map(|cert| cert.to_vec()).collect());
    }

    Certificate::from_der(&pem_or_der).map_err(|err| {
        CliError::Config(format!(
            "vpn.ca_cert {} is neither a PEM certificate bundle nor a valid DER certificate: {err}",
            path.display()
        ))
    })?;
    Ok(vec![pem_or_der])
}

fn looks_like_pem_certificate_bundle(contents: &[u8]) -> bool {
    let trimmed = contents
        .iter()
        .copied()
        .skip_while(|byte| byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    trimmed.starts_with(b"-----BEGIN CERTIFICATE-----")
}

pub fn load(path: impl AsRef<Path>) -> Result<AppConfig, String> {
    load_typed(path).map_err(|err| err.to_string())
}

pub fn load_typed(path: impl AsRef<Path>) -> Result<AppConfig, CliError> {
    let body = std::fs::read_to_string(path).map_err(|err| CliError::Config(err.to_string()))?;
    toml::from_str(&body).map_err(|err| CliError::Config(err.to_string()))
}

#[cfg(any(test, feature = "test-utils"))]
pub fn load_for_test(path: impl AsRef<Path>) -> Result<AppConfig, String> {
    let body = fs::read_to_string(path).map_err(|err| err.to_string())?;
    toml::from_str(&body).map_err(|err| err.to_string())
}

#[cfg(any(test, feature = "test-utils"))]
pub fn merge_for_test<const N: usize>(
    path: impl AsRef<Path>,
    args: [&str; N],
) -> Result<AppConfig, String> {
    let mut cfg = load_for_test(path)?;
    let cli = crate::cli::Cli::parse_from(
        std::iter::once("smelly-connect-cli")
            .chain(std::iter::once("proxy"))
            .chain(args),
    );
    let crate::cli::Command::Proxy(command) = cli.command else {
        return Err("expected proxy command".to_string());
    };
    apply_proxy_overrides(&mut cfg, &command);
    Ok(cfg)
}

pub fn merge_proxy_command(
    path: impl AsRef<Path>,
    command: &ProxyCommand,
) -> Result<AppConfig, String> {
    let mut cfg = load_typed(path).map_err(|err| err.to_string())?;
    apply_proxy_overrides(&mut cfg, command);
    Ok(cfg)
}

pub fn merge_proxy_command_typed(
    path: impl AsRef<Path>,
    command: &ProxyCommand,
) -> Result<AppConfig, CliError> {
    let mut cfg = load_typed(path)?;
    apply_proxy_overrides(&mut cfg, command);
    Ok(cfg)
}

pub fn apply_proxy_overrides(cfg: &mut AppConfig, command: &ProxyCommand) {
    if let Some(min_pool_size) = command.min_pool_size {
        cfg.pool.min_pool_size = min_pool_size;
    }
    if let Some(listen_http) = &command.listen_http {
        cfg.proxy.http.listen = listen_http.clone();
    }
    if let Some(listen_socks5) = &command.listen_socks5 {
        cfg.proxy.socks5.listen = listen_socks5.clone();
    }
    if let Some(keepalive_host) = &command.keepalive_host {
        cfg.vpn.default_keepalive_host = Some(keepalive_host.clone());
    }
    if command.allow_all {
        cfg.routing.allow_all = true;
    }
}
