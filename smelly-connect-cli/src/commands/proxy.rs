use crate::cli::ProxyCommand;
use std::path::Path;
use std::time::Duration;

pub async fn run_proxy(
    config_path: impl AsRef<Path>,
    command: &ProxyCommand,
) -> Result<(), String> {
    let config = crate::config::merge_proxy_command(config_path, command)?;
    let pool = crate::pool::SessionPool::from_config_allow_empty(&config)
        .await
        .map_err(|err| err.to_string())?;
    let stats = crate::runtime::RuntimeStats::default();
    let connect_timeout = Duration::from_secs(config.pool.connect_timeout_secs.max(1));
    let ready = pool.ready_count().await;
    tracing::info!(
        ready,
        http_enabled = config.proxy.http.enabled,
        socks5_enabled = config.proxy.socks5.enabled,
        "starting proxy service"
    );

    let mut tasks = Vec::new();
    if config.proxy.http.enabled {
        let listen_http = config.proxy.http.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let connect_timeout = connect_timeout;
        tasks.push(tokio::spawn(crate::proxy::http::serve_http(
            listen_http,
            pool,
            stats,
            connect_timeout,
        )));
    }
    if config.proxy.socks5.enabled {
        let listen_socks5 = config.proxy.socks5.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let connect_timeout = connect_timeout;
        tasks.push(tokio::spawn(crate::proxy::socks5::serve_socks5(
            listen_socks5,
            pool,
            stats,
            connect_timeout,
        )));
    }
    #[cfg(feature = "management-api")]
    if config.management.enabled {
        let listen_management = config.management.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        tasks.push(tokio::spawn(crate::management::serve_management(
            listen_management,
            pool,
            stats,
        )));
    }

    #[cfg(not(feature = "management-api"))]
    if config.management.enabled {
        return Err(
            "management api requested in config but this binary was built without the management-api feature"
                .to_string(),
        );
    }

    if tasks.is_empty() {
        return Err("no proxy listener enabled".to_string());
    }

    for task in tasks {
        task.await.map_err(|err| err.to_string())??;
    }
    Ok(())
}
