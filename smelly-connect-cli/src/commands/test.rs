use std::path::Path;
#[cfg(any(test, debug_assertions))]
use std::sync::Arc;
#[cfg(any(test, debug_assertions))]
use std::sync::atomic::AtomicUsize;

#[cfg(any(test, debug_assertions))]
use smelly_connect::test_support;

use crate::error::CliError;

#[cfg(any(test, debug_assertions))]
pub async fn run_tcp_for_test(target: &str) -> Result<String, String> {
    let session = test_support::session::login_harness().ready_session().await;
    let (host, port) = split_target(target)?;
    let _stream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(format!("tcp ok: {host}:{port}"))
}

#[cfg(any(test, debug_assertions))]
pub async fn run_icmp_for_test(target: &str) -> Result<String, String> {
    let counter = Arc::new(AtomicUsize::new(0));
    let session = test_support::session::session_with_icmp_ping(counter);
    session
        .icmp_ping(target.into())
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(format!("icmp ok: {target}"))
}

#[cfg(any(test, debug_assertions))]
pub async fn run_http_for_test(url: &str) -> Result<String, String> {
    let harness = test_support::integration::reqwest_harness().await;
    let client = harness
        .session
        .reqwest_client()
        .await
        .map_err(|err| format!("{err:?}"))?;
    let body = harness.get_with(client, url).await;
    Ok(format!("status=200 body={body}"))
}

pub async fn run_tcp(target: &str) -> Result<(), String> {
    let output = run_tcp_with_config("config.toml", target).await?;
    println!("{output}");
    Ok(())
}

pub async fn run_icmp(target: &str) -> Result<(), String> {
    let output = run_icmp_with_config("config.toml", target).await?;
    println!("{output}");
    Ok(())
}

pub async fn run_http(url: &str) -> Result<(), String> {
    let output = run_http_with_config("config.toml", url).await?;
    println!("{output}");
    Ok(())
}

pub async fn run_tcp_with_config(
    config_path: impl AsRef<Path>,
    target: &str,
) -> Result<String, String> {
    run_tcp_with_config_typed(config_path, target)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_tcp_with_config_typed(
    config_path: impl AsRef<Path>,
    target: &str,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let pool = crate::pool::SessionPool::from_config(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let (_account_name, session) = pool
        .next_live_session()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let (host, port) = split_target_typed(target)?;
    let _stream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(|err| CliError::Command(format!("{err:?}")))?;
    Ok(format!("tcp ok: {host}:{port}"))
}

pub async fn run_icmp_with_config(
    config_path: impl AsRef<Path>,
    target: &str,
) -> Result<String, String> {
    run_icmp_with_config_typed(config_path, target)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_icmp_with_config_typed(
    config_path: impl AsRef<Path>,
    target: &str,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let pool = crate::pool::SessionPool::from_config(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let (_account_name, session) = pool
        .next_live_session()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    session
        .icmp_ping(target.into())
        .await
        .map_err(|err| CliError::Command(format!("{err:?}")))?;
    Ok(format!("icmp ok: {target}"))
}

pub async fn run_http_with_config(
    config_path: impl AsRef<Path>,
    url: &str,
) -> Result<String, String> {
    run_http_with_config_typed(config_path, url)
        .await
        .map_err(|err| err.to_string())
}

pub async fn run_http_with_config_typed(
    config_path: impl AsRef<Path>,
    url: &str,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let pool = crate::pool::SessionPool::from_config(&config)
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let (_account_name, session) = pool
        .next_live_session()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let client = session
        .reqwest_client()
        .await
        .map_err(|err| CliError::Command(format!("{err:?}")))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| CliError::Command(err.to_string()))?;
    let body_len = body.len();
    let has_html = body.to_ascii_lowercase().contains("<html");
    Ok(format!(
        "status={status} body_len={body_len} html={has_html}"
    ))
}

fn split_target(target: &str) -> Result<(String, u16), String> {
    split_target_typed(target).map_err(|err| err.to_string())
}

fn split_target_typed(target: &str) -> Result<(String, u16), CliError> {
    let (host, port) = target
        .rsplit_once(':')
        .ok_or_else(|| CliError::Command("missing :port".to_string()))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| CliError::Command("invalid port".to_string()))?;
    Ok((host.to_string(), port))
}
