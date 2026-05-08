use std::path::Path;
#[cfg(feature = "test-utils")]
use std::sync::Arc;
#[cfg(feature = "test-utils")]
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

#[cfg(feature = "test-utils")]
use smelly_connect::test_support;

use crate::error::CliError;

#[cfg(feature = "test-utils")]
pub async fn run_tcp_for_test(target: &str) -> Result<String, String> {
    let session = test_support::session::login_harness().ready_session().await;
    let (host, port) = split_target(target)?;
    let _stream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(format!("tcp ok: {host}:{port}"))
}

#[cfg(feature = "test-utils")]
pub async fn run_icmp_for_test(target: &str) -> Result<String, String> {
    let counter = Arc::new(AtomicUsize::new(0));
    let session = test_support::session::session_with_icmp_ping(counter);
    session
        .icmp_ping(target.into())
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(format!("icmp ok: {target}"))
}

#[cfg(feature = "test-utils")]
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

pub async fn run_legacy_probe() -> Result<(), String> {
    let output = run_legacy_probe_with_config("config.toml").await?;
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

pub async fn run_legacy_probe_with_config(config_path: impl AsRef<Path>) -> Result<String, String> {
    run_legacy_probe_with_config_typed(config_path)
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

pub async fn run_legacy_probe_with_config_typed(
    config_path: impl AsRef<Path>,
) -> Result<String, CliError> {
    let config = crate::config::load_typed(config_path)?;
    let account = config
        .accounts
        .first()
        .ok_or_else(|| CliError::Command("no account configured".to_string()))?;
    let cfg = smelly_connect::EasyConnectConfig::new(
        config.vpn.server.clone(),
        account.username.clone(),
        account.password.clone(),
    )
    .with_server_cert_policy(config.server_cert_policy())
    .with_captcha_handler(smelly_connect::CaptchaHandler::from_async(
        |_, _| async move {
            Err(smelly_connect::CaptchaError::new(
                "captcha callback not configured for legacy probe",
            ))
        },
    ));

    let state = smelly_connect::run_control_plane(&cfg)
        .await
        .map_err(|err| CliError::Command(format!("{err:?}")))?;
    let token = smelly_connect::auth::control::request_token_async_with_policy(
        &config.vpn.server,
        &state.authorized_twfid,
        config.server_cert_policy(),
    )
    .await
    .map_err(|err| CliError::Command(format!("{err:?}")))?;
    let addr = tokio::net::lookup_host((config.vpn.server.as_str(), 443))
        .await
        .map_err(|err| CliError::Command(err.to_string()))?
        .next()
        .ok_or_else(|| CliError::Command("no resolved server address".to_string()))?;
    let hint = state.legacy_cipher_hint.as_deref();
    let timeout = Duration::from_secs(5);

    let mut lines = Vec::new();
    lines.push(format!("server_addr={addr}"));
    lines.push(format!("legacy_cipher_hint={hint:?}"));

    let preconnect_request_ip = run_probe_step(timeout, async {
        smelly_connect::auth::control::request_ip_for_server_with_policy(
            &config.vpn.server,
            &token,
            hint,
            config.server_cert_policy(),
        )
        .await
    })
    .await;
    lines.push(format!("preconnect_request_ip: {preconnect_request_ip}"));

    let preconnect_recv = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_recv_tunnel_for_server_with_policy(
            &config.vpn.server,
            &token,
            "10.0.0.8".parse().unwrap(),
            hint,
            config.server_cert_policy(),
        )
        .await
        .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!("preconnect_open_recv: {preconnect_recv}"));

    let preconnect_send = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_send_tunnel_for_server_with_policy(
            &config.vpn.server,
            &token,
            "10.0.0.8".parse().unwrap(),
            hint,
            config.server_cert_policy(),
        )
        .await
        .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!("preconnect_open_send: {preconnect_send}"));

    let preconnect_hold_request_ip_and_open_recv = run_probe_step(timeout, async {
        let ip = smelly_connect::auth::control::request_ip_for_server_with_policy(
            &config.vpn.server,
            &token,
            hint,
            config.server_cert_policy(),
        )
        .await?;
        smelly_connect::auth::control::open_recv_tunnel_for_server_with_policy(
            &config.vpn.server,
            &token,
            ip,
            hint,
            config.server_cert_policy(),
        )
        .await
        .map(|_| format!("ok ip={ip}"))
    })
    .await;
    lines.push(format!(
        "preconnect_hold_request_ip_then_open_recv: {preconnect_hold_request_ip_and_open_recv}"
    ));

    let preconnect_hold_request_ip_and_open_send = run_probe_step(timeout, async {
        let (ip, _conn) = smelly_connect::auth::control::request_ip_via_tunnel_with_conn_debug(
            addr, &token, hint,
        )
        .await?;
        smelly_connect::auth::control::open_send_tunnel(addr, &token, ip, hint)
            .await
            .map(|_| format!("ok ip={ip}"))
    })
    .await;
    lines.push(format!(
        "preconnect_hold_request_ip_then_open_send: {preconnect_hold_request_ip_and_open_send}"
    ));

    let preconnect_second_pair_same_lease = run_probe_step(timeout, async {
        let (ip, _lease) = smelly_connect::auth::control::request_ip_via_tunnel_with_conn_debug(
            addr, &token, hint,
        )
        .await?;
        let recv1 = smelly_connect::auth::control::open_recv_tunnel(addr, &token, ip, hint).await?;
        let send1 = smelly_connect::auth::control::open_send_tunnel(addr, &token, ip, hint).await?;
        let recv2 = smelly_connect::auth::control::open_recv_tunnel(addr, &token, ip, hint)
            .await
            .map(|_| "ok".to_string())
            .map_err(|err| format!("{err:?}"));
        let send2 = smelly_connect::auth::control::open_send_tunnel(addr, &token, ip, hint)
            .await
            .map(|_| "ok".to_string())
            .map_err(|err| format!("{err:?}"));
        drop(recv1);
        drop(send1);
        Ok::<_, smelly_connect::Error>(format!("recv2={recv2:?} send2={send2:?} ip={ip}"))
    })
    .await;
    lines.push(format!(
        "preconnect_second_pair_same_lease: {preconnect_second_pair_same_lease}"
    ));

    let preconnect_second_pair_after_drop = run_probe_step(timeout, async {
        let (ip, _lease) = smelly_connect::auth::control::request_ip_via_tunnel_with_conn_debug(
            addr, &token, hint,
        )
        .await?;
        let recv1 = smelly_connect::auth::control::open_recv_tunnel(addr, &token, ip, hint).await?;
        let send1 = smelly_connect::auth::control::open_send_tunnel(addr, &token, ip, hint).await?;
        drop(recv1);
        drop(send1);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let recv2 = smelly_connect::auth::control::open_recv_tunnel(addr, &token, ip, hint)
            .await
            .map(|_| "ok".to_string())
            .map_err(|err| format!("{err:?}"));
        let send2 = smelly_connect::auth::control::open_send_tunnel(addr, &token, ip, hint)
            .await
            .map(|_| "ok".to_string())
            .map_err(|err| format!("{err:?}"));
        Ok::<_, smelly_connect::Error>(format!("recv2={recv2:?} send2={send2:?} ip={ip}"))
    })
    .await;
    lines.push(format!(
        "preconnect_second_pair_after_drop: {preconnect_second_pair_after_drop}"
    ));

    let full_session = cfg
        .clone()
        .connect()
        .await
        .map_err(|err| CliError::Command(format!("{err:?}")))?
        .with_allow_all_routes(true);
    let session_client_ip = full_session.client_ip();
    lines.push(format!("session_client_ip={session_client_ip}"));

    let session_authserver_timeout = run_probe_step(timeout, async {
        full_session
            .connect_tcp(("authserver.sit.edu.cn", 443))
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!(
        "same_session_authserver_connect: {session_authserver_timeout}"
    ));

    let session_icmp_after_timeout = run_probe_step(timeout, async {
        full_session
            .icmp_ping("jwxt.sit.edu.cn".into())
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!(
        "same_session_icmp_after_authserver_timeout: {session_icmp_after_timeout}"
    ));

    let session_jwxt_after_timeout = run_probe_step(timeout, async {
        full_session
            .connect_tcp(("jwxt.sit.edu.cn", 443))
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!(
        "same_session_jwxt_connect_after_authserver_timeout: {session_jwxt_after_timeout}"
    ));

    let session_xg_after_timeout = run_probe_step(timeout, async {
        full_session
            .connect_tcp(("xg.sit.edu.cn", 443))
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!(
        "same_session_xg_connect_after_authserver_timeout: {session_xg_after_timeout}"
    ));

    let postconnect_request_ip = run_probe_step(timeout, async {
        smelly_connect::auth::control::request_ip_via_tunnel(addr, &token, hint).await
    })
    .await;
    lines.push(format!("postconnect_request_ip: {postconnect_request_ip}"));

    let postconnect_recv = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_recv_tunnel(addr, &token, session_client_ip, hint)
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!(
        "postconnect_open_recv_with_session_ip: {postconnect_recv}"
    ));

    let postconnect_send = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_send_tunnel(addr, &token, session_client_ip, hint)
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!(
        "postconnect_open_send_with_session_ip: {postconnect_send}"
    ));

    let same_conn_recv = run_probe_step(timeout, async {
        let (ip, mut conn) = smelly_connect::auth::control::request_ip_via_tunnel_with_conn_debug(
            addr, &token, hint,
        )
        .await?;
        let payload = smelly_connect::protocol::build_recv_handshake(&token, ip);
        conn.send_application_data(&payload).await.map_err(|err| {
            smelly_connect::Error::TunnelBootstrap(
                smelly_connect::error::TunnelBootstrapError::HandshakeFailed(err.to_string()),
            )
        })?;
        let reply = conn.read_application_data().await.map_err(|err| {
            smelly_connect::Error::TunnelBootstrap(
                smelly_connect::error::TunnelBootstrapError::HandshakeFailed(err.to_string()),
            )
        })?;
        Ok::<_, smelly_connect::Error>(format!(
            "reply=0x{:02x} len={} ip={ip}",
            reply.first().copied().unwrap_or_default(),
            reply.len()
        ))
    })
    .await;
    lines.push(format!("same_conn_recv_after_request_ip: {same_conn_recv}"));

    let same_conn_send = run_probe_step(timeout, async {
        let (ip, mut conn) = smelly_connect::auth::control::request_ip_via_tunnel_with_conn_debug(
            addr, &token, hint,
        )
        .await?;
        let payload = smelly_connect::protocol::build_send_handshake(&token, ip);
        conn.send_application_data(&payload).await.map_err(|err| {
            smelly_connect::Error::TunnelBootstrap(
                smelly_connect::error::TunnelBootstrapError::HandshakeFailed(err.to_string()),
            )
        })?;
        let reply = conn.read_application_data().await.map_err(|err| {
            smelly_connect::Error::TunnelBootstrap(
                smelly_connect::error::TunnelBootstrapError::HandshakeFailed(err.to_string()),
            )
        })?;
        Ok::<_, smelly_connect::Error>(format!(
            "reply=0x{:02x} len={} ip={ip}",
            reply.first().copied().unwrap_or_default(),
            reply.len()
        ))
    })
    .await;
    lines.push(format!("same_conn_send_after_request_ip: {same_conn_send}"));

    drop(full_session);
    tokio::time::sleep(Duration::from_secs(2)).await;

    let postdrop_request_ip = run_probe_step(timeout, async {
        smelly_connect::auth::control::request_ip_via_tunnel(addr, &token, hint).await
    })
    .await;
    lines.push(format!("postdrop_request_ip: {postdrop_request_ip}"));

    let postdrop_open_recv = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_recv_tunnel(addr, &token, session_client_ip, hint)
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!("postdrop_open_recv: {postdrop_open_recv}"));

    let postdrop_open_send = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_send_tunnel(addr, &token, session_client_ip, hint)
            .await
            .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!("postdrop_open_send: {postdrop_open_send}"));

    let refreshed_token = smelly_connect::auth::control::request_token_async(
        &config.vpn.server,
        &state.authorized_twfid,
    )
    .await
    .map_err(|err| CliError::Command(format!("{err:?}")))?;
    lines.push("refreshed_token=ok".to_string());

    let refreshed_request_ip = run_probe_step(timeout, async {
        smelly_connect::auth::control::request_ip_via_tunnel(addr, &refreshed_token, hint).await
    })
    .await;
    lines.push(format!(
        "refreshed_token_request_ip: {refreshed_request_ip}"
    ));

    let refreshed_open_recv = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_recv_tunnel(
            addr,
            &refreshed_token,
            session_client_ip,
            hint,
        )
        .await
        .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!("refreshed_token_open_recv: {refreshed_open_recv}"));

    let refreshed_open_send = run_probe_step(timeout, async {
        smelly_connect::auth::control::open_send_tunnel(
            addr,
            &refreshed_token,
            session_client_ip,
            hint,
        )
        .await
        .map(|_| "ok".to_string())
    })
    .await;
    lines.push(format!("refreshed_token_open_send: {refreshed_open_send}"));

    Ok(lines.join("\n"))
}

async fn run_probe_step<F, T, E>(timeout: Duration, fut: F) -> String
where
    F: std::future::Future<Output = Result<T, E>>,
    T: std::fmt::Display,
    E: std::fmt::Debug,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => format!("ok {value}"),
        Ok(Err(err)) => format!("err {err:?}"),
        Err(_) => "timeout".to_string(),
    }
}

#[cfg(feature = "test-utils")]
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
