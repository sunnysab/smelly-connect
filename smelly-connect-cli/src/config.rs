use serde::Deserialize;

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
