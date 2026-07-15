use crate::config::AppConfig;
use crate::error::CliError;

pub async fn run_route(config: &AppConfig, host: &str, port: u16) -> Result<String, CliError> {
    let pool = crate::pool::SessionPool::from_config(config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let pooled = pool
        .acquire()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let session = pooled
        .session()
        .cloned()
        .ok_or_else(|| CliError::Command("acquired session with no inner session".to_string()))?;
    match session.plan_tcp_connect((host, port)).await {
        Ok(route) => Ok(format!("allowed: {route:?}")),
        Err(err) => Ok(format!("rejected: {err:?}")),
    }
}

pub async fn run_session(config: &AppConfig) -> Result<String, CliError> {
    let pool = crate::pool::SessionPool::from_config(config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let ready = pool.ready_count().await;
    Ok(format!(
        "configured={} ready={ready}",
        config.accounts.len()
    ))
}
