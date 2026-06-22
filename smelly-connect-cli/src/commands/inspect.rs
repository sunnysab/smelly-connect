use std::path::Path;

use crate::error::CliError;

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

#[cfg(feature = "test-utils")]
mod tests {
    use smelly_connect::test_support;
    use smelly_connect::{resolver::SessionResolver, resource::ResourceSet};

    pub async fn inspect_route_for_test(host: &str, port: u16) -> String {
        let session = test_support::session::login_harness().ready_session().await;
        format_route_decision(&session, host, port).await
    }

    pub async fn inspect_unmatched_route_for_test(host: &str, port: u16) -> String {
        let session = unmatched_session_for_test(
            smelly_connect::domain::route_policy::RoutePolicy::direct_non_resource_targets(),
        );
        format_route_decision(&session, host, port).await
    }

    pub async fn inspect_unmatched_blocked_route_for_test(host: &str, port: u16) -> String {
        let session = unmatched_session_for_test(
            smelly_connect::domain::route_policy::RoutePolicy::block_non_resource_targets(),
        );
        format_route_decision(&session, host, port).await
    }

    async fn format_route_decision(
        session: &smelly_connect::session::EasyConnectSession,
        host: &str,
        port: u16,
    ) -> String {
        match session.plan_tcp_connect((host, port)).await {
            Ok(route) => format!("allowed: {route:?}"),
            Err(err) => format!("rejected: {err:?}"),
        }
    }

    fn unmatched_session_for_test(
        route_policy: smelly_connect::domain::route_policy::RoutePolicy,
    ) -> smelly_connect::session::EasyConnectSession {
        let mut system_dns = std::collections::HashMap::new();
        system_dns.insert(
            "example.test".to_string(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );
        smelly_connect::session::EasyConnectSession::new(
            "10.0.0.8".parse().unwrap(),
            ResourceSet::default(),
            SessionResolver::new(std::collections::HashMap::new(), None, system_dns),
            smelly_connect::session::EasyConnectSession::failing_transport("unused"),
        )
        .with_route_policy(route_policy)
    }

    pub async fn inspect_session_for_test() -> String {
        let pool =
            crate::pool::SessionPool::from_named_ready_accounts(["acct-01", "acct-02"]).await;
        let ready = pool.ready_count().await;
        format!("ready={ready}")
    }
}

#[cfg(feature = "test-utils")]
pub use tests::*;
