use crate::cli::ProxyCommand;
use crate::config::AppConfig;
use crate::error::CliError;
use std::future::Future;
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};

pub async fn run_proxy<F>(
    config: &AppConfig,
    command: &ProxyCommand,
    shutdown: F,
) -> Result<(), CliError>
where
    F: Future<Output = ()> + Send,
{
    let mut config = config.clone();
    crate::config::apply_proxy_overrides(&mut config, command);
    let pool = crate::pool::SessionPool::from_config_allow_empty(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let stats = crate::runtime::RuntimeStats::default();
    let upstream_tcp_connect_timeout = config.upstream_tcp_connect_timeout();
    let udp_associate_idle_timeout = config.udp_associate_idle_timeout();
    let shutdown_drain_timeout = config.shutdown_drain_timeout();
    let ready = pool.ready_count().await;
    tracing::info!(
        ready,
        http_enabled = config.proxy.http.enabled,
        socks5_enabled = config.proxy.socks5.enabled,
        shutdown_drain_timeout_ms = shutdown_drain_timeout.as_millis() as u64,
        "starting proxy service"
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::pin!(shutdown);
    let mut tasks = tokio::task::JoinSet::new();
    if config.proxy.http.enabled {
        let listen_http = config.proxy.http.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let shutdown = shutdown_rx.clone();
        tasks.spawn(async move {
            crate::proxy::http::serve_http_with_shutdown(
                listen_http,
                pool,
                stats,
                upstream_tcp_connect_timeout,
                shutdown,
            )
            .await
            .map_err(|err| format!("http listener failed: {err}"))
        });
    }
    if config.proxy.socks5.enabled {
        let listen_socks5 = config.proxy.socks5.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let shutdown = shutdown_rx.clone();
        tasks.spawn(async move {
            crate::proxy::socks5::serve_socks5_with_shutdown(
                listen_socks5,
                pool,
                stats,
                upstream_tcp_connect_timeout,
                udp_associate_idle_timeout,
                shutdown,
            )
            .await
            .map_err(|err| format!("socks5 listener failed: {err}"))
        });
    }
    #[cfg(feature = "management-api")]
    if config.management.enabled {
        let listen_management = config.management.listen.clone();
        let pool = pool.clone();
        let stats = stats.clone();
        let shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(err) =
                crate::management::serve_management(listen_management, pool, stats, shutdown).await
            {
                tracing::warn!(error = %err, "management listener failed, continuing without management API");
            }
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

    let mut shutdown_requested = false;
    let mut forced_abort = false;
    let mut first_error: Option<String> = None;
    let mut drain_deadline: Option<Instant> = None;
    while !tasks.is_empty() {
        tokio::select! {
            _ = &mut shutdown, if !shutdown_requested => {
                shutdown_requested = true;
                let _ = shutdown_tx.send(true);
                drain_deadline = Some(Instant::now() + shutdown_drain_timeout);
            }
            _ = async {
                if let Some(deadline) = drain_deadline {
                    sleep_until(deadline).await;
                }
            }, if shutdown_requested && !forced_abort && drain_deadline.is_some() => {
                forced_abort = true;
                tasks.abort_all();
            }
            result = tasks.join_next(), if !tasks.is_empty() => {
                let Some(result) = result else {
                    break;
                };
                match result {
                    Ok(Ok(())) => {
                        if !shutdown_requested {
                            shutdown_requested = true;
                            first_error = Some("proxy listener exited unexpectedly".to_string());
                            let _ = shutdown_tx.send(true);
                            drain_deadline = Some(Instant::now() + shutdown_drain_timeout);
                        }
                    }
                    Ok(Err(err)) => {
                        if first_error.is_none() {
                            first_error = Some(err);
                        }
                        if !shutdown_requested {
                            shutdown_requested = true;
                            let _ = shutdown_tx.send(true);
                            drain_deadline = Some(Instant::now() + shutdown_drain_timeout);
                        }
                    }
                    Err(err) => {
                        if err.is_cancelled() && forced_abort {
                            continue;
                        }
                        if first_error.is_none() {
                            first_error = Some(format!("proxy listener task failed: {err}"));
                        }
                        if !shutdown_requested {
                            shutdown_requested = true;
                            let _ = shutdown_tx.send(true);
                            drain_deadline = Some(Instant::now() + shutdown_drain_timeout);
                        }
                    }
                }
            }
        }
    }

    if forced_abort {
        tracing::warn!(
            shutdown_drain_timeout_ms = shutdown_drain_timeout.as_millis() as u64,
            "proxy shutdown exceeded drain timeout; aborting remaining listener tasks"
        );
    }

    pool.shutdown().await;

    match first_error {
        Some(err) => Err(CliError::Command(err)),
        None => Ok(()),
    }
}
