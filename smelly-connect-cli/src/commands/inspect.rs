#[cfg(any(test, debug_assertions))]
use smelly_connect::test_support;
use std::path::Path;

use crate::error::CliError;

#[cfg(any(test, debug_assertions))]
pub async fn inspect_route_for_test(host: &str, port: u16) -> String {
    let session = test_support::session::login_harness().ready_session().await;
    match session.plan_tcp_connect((host, port)).await {
        Ok(route) => format!("allowed: {route:?}"),
        Err(err) => format!("rejected: {err:?}"),
    }
}

#[cfg(any(test, debug_assertions))]
pub async fn inspect_session_for_test() -> String {
    let pool = crate::pool::SessionPool::from_named_ready_accounts(["acct-01", "acct-02"]).await;
    let ready = pool.ready_count().await;
    format!("ready={ready}")
}

pub async fn run_route(host: &str, port: u16) -> Result<(), String> {
    let output = run_route_with_config("config.toml", host, port).await?;
    println!("{output}");
    Ok(())
}

pub async fn run_session() -> Result<(), String> {
    let output = run_session_with_config("config.toml").await?;
    println!("{output}");
    Ok(())
}

pub async fn run_route_with_config(
    config_path: impl AsRef<Path>,
    host: &str,
    port: u16,
) -> Result<String, String> {
    run_route_with_config_typed(config_path, host, port)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_route_with_config_typed(
    config_path: impl AsRef<Path>,
    host: &str,
    port: u16,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let pool = crate::pool::SessionPool::from_config(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let (_account_name, session) = pool
        .next_live_session()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    match session.plan_tcp_connect((host, port)).await {
        Ok(route) => Ok(format!("allowed: {route:?}")),
        Err(err) => Ok(format!("rejected: {err:?}")),
    }
}

pub async fn run_session_with_config(config_path: impl AsRef<Path>) -> Result<String, String> {
    run_session_with_config_typed(config_path)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_session_with_config_typed(
    config_path: impl AsRef<Path>,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let pool = crate::pool::SessionPool::from_config(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let ready = pool.ready_count().await;
    Ok(format!(
        "configured={} ready={ready}",
        config.accounts.len()
    ))
}
