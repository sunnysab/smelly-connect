use crate::cli::ProxyCommand;
use crate::error::CliError;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Notify;

pub async fn run_proxy(
    config_path: impl AsRef<Path>,
    command: &ProxyCommand,
) -> Result<(), String> {
    run_proxy_typed(config_path, command)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_proxy_typed(
    config_path: impl AsRef<Path>,
    command: &ProxyCommand,
) -> Result<(), CliError> {
    let config = crate::config::merge_proxy_command_typed(config_path, command)?;
    let pool = crate::pool::SessionPool::from_config_allow_empty(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let stats = crate::runtime::RuntimeStats::default();
    let upstream_tcp_connect_timeout = config.upstream_tcp_connect_timeout();
    let udp_associate_idle_timeout = config.udp_associate_idle_timeout();
    let ready = pool.ready_count().await;
    tracing::info!(
        ready,
        http_enabled = config.proxy.http.enabled,
        socks5_enabled = config.proxy.socks5.enabled,
        "starting proxy service"
    );

    let shutdown = Arc::new(Notify::new());
    let mut tasks = tokio::task::JoinSet::new();
    if config.proxy.http.enabled {
        let listen_http = config.proxy.http.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let shutdown = shutdown.clone();
        tasks.spawn(async move {
            let result = crate::proxy::http::serve_http(
                listen_http, pool, stats, upstream_tcp_connect_timeout,
            )
            .await;
            shutdown.notify_one();
            result.map_err(|err| format!("http listener failed: {err}"))
        });
    }
    if config.proxy.socks5.enabled {
        let listen_socks5 = config.proxy.socks5.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let shutdown = shutdown.clone();
        tasks.spawn(async move {
            let result = crate::proxy::socks5::serve_socks5(
                listen_socks5,
                pool,
                stats,
                upstream_tcp_connect_timeout,
                udp_associate_idle_timeout,
            )
            .await;
            shutdown.notify_one();
            result.map_err(|err| format!("socks5 listener failed: {err}"))
        });
    }
    #[cfg(feature = "management-api")]
    if config.management.enabled {
        let listen_management = config.management.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        tasks.spawn(async move {
            crate::management::serve_management(listen_management, pool, stats)
                .await
                .map_err(|err| format!("management listener failed: {err}"))
        });
    }

    #[cfg(not(feature = "management-api"))]
    if config.management.enabled {
        return Err(CliError::Command(
            "management api requested in config but this binary was built without the management-api feature"
                .to_string(),
        ));
    }

    if tasks.is_empty() {
        return Err(CliError::Command("no proxy listener enabled".to_string()));
    }

    // Wait for first listener to finish
    let Some(result) = tasks.join_next().await else {
        return Err(CliError::Command(
            "no proxy listener remained running".to_string(),
        ));
    };

    // Notify other listeners to shut down too
    shutdown.notify_waiters();

    match result {
        Ok(Ok(())) => Err(CliError::Command(
            "proxy listener exited unexpectedly".to_string(),
        )),
        Ok(Err(err)) => Err(CliError::Command(err)),
        Err(err) => Err(CliError::Command(format!(
            "proxy listener task failed: {err}"
        ))),
    }
}
