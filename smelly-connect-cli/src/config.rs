use serde::Deserialize;
use std::fs;
use std::path::Path;

use crate::cli::ProxyCommand;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub vpn: VpnConfig,
    pub pool: PoolConfig,
    pub accounts: Vec<AccountConfig>,
    pub proxy: ProxyConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VpnConfig {
    pub server: String,
    pub default_keepalive_host: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PoolConfig {
    pub prewarm: usize,
    pub connect_timeout_secs: u64,
    pub healthcheck_interval_secs: u64,
    pub selection: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    pub http: ListenerConfig,
    pub socks5: ListenerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerConfig {
    pub enabled: bool,
    pub listen: String,
}

pub fn load(path: impl AsRef<Path>) -> Result<AppConfig, String> {
    load_for_test(path)
}

pub fn load_for_test(path: impl AsRef<Path>) -> Result<AppConfig, String> {
    let body = fs::read_to_string(path).map_err(|err| err.to_string())?;
    toml::from_str(&body).map_err(|err| err.to_string())
}

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
    let mut cfg = load(path)?;
    apply_proxy_overrides(&mut cfg, command);
    Ok(cfg)
}

pub fn apply_proxy_overrides(cfg: &mut AppConfig, command: &ProxyCommand) {
    if let Some(prewarm) = command.prewarm {
        cfg.pool.prewarm = prewarm;
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
}
