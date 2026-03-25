use crate::cli::ProxyCommand;
use std::path::Path;

pub async fn run_proxy(
    config_path: impl AsRef<Path>,
    command: &ProxyCommand,
) -> Result<(), String> {
    let config = crate::config::merge_proxy_command(config_path, command)?;
    let pool = crate::pool::SessionPool::from_config(&config)
        .await
        .map_err(|err| err.to_string())?;
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
        tasks.push(tokio::spawn(crate::proxy::http::serve_http(
            listen_http,
            pool,
        )));
    }
    if config.proxy.socks5.enabled {
        let listen_socks5 = config.proxy.socks5.listen.clone();
        let pool = pool.clone();
        tasks.push(tokio::spawn(crate::proxy::socks5::serve_socks5(
            listen_socks5,
            pool,
        )));
    }

    if tasks.is_empty() {
        return Err("no proxy listener enabled".to_string());
    }

    for task in tasks {
        task.await.map_err(|err| err.to_string())??;
    }
    Ok(())
}
