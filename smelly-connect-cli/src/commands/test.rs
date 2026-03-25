use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use smelly_connect::test_support;

pub async fn run_tcp_for_test(target: &str) -> Result<String, String> {
    let session = test_support::session::login_harness().ready_session().await;
    let (host, port) = split_target(target)?;
    let _stream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(format!("tcp ok: {host}:{port}"))
}

pub async fn run_icmp_for_test(target: &str) -> Result<String, String> {
    let counter = Arc::new(AtomicUsize::new(0));
    let session = test_support::session::session_with_icmp_ping(counter);
    session
        .icmp_ping(target.into())
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(format!("icmp ok: {target}"))
}

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
    println!("{}", run_tcp_for_test(target).await?);
    Ok(())
}

pub async fn run_icmp(target: &str) -> Result<(), String> {
    println!("{}", run_icmp_for_test(target).await?);
    Ok(())
}

pub async fn run_http(url: &str) -> Result<(), String> {
    println!("{}", run_http_for_test(url).await?);
    Ok(())
}

fn split_target(target: &str) -> Result<(String, u16), String> {
    let (host, port) = target
        .rsplit_once(':')
        .ok_or_else(|| "missing :port".to_string())?;
    let port = port
        .parse::<u16>()
        .map_err(|_| "invalid port".to_string())?;
    Ok((host.to_string(), port))
}
