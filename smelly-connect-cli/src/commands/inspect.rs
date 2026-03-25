use smelly_connect::test_support;

pub async fn inspect_route_for_test(host: &str, port: u16) -> String {
    let session = test_support::session::login_harness().ready_session().await;
    match session.plan_tcp_connect((host, port)).await {
        Ok(route) => format!("allowed: {route:?}"),
        Err(err) => format!("rejected: {err:?}"),
    }
}

pub async fn inspect_session_for_test() -> String {
    let pool = crate::pool::SessionPool::from_named_ready_accounts(["acct-01", "acct-02"]).await;
    let ready = pool.ready_count().await;
    format!("ready={ready}")
}

pub async fn run_route(host: &str, port: u16) -> Result<(), String> {
    println!("{}", inspect_route_for_test(host, port).await);
    Ok(())
}

pub async fn run_session() -> Result<(), String> {
    println!("{}", inspect_session_for_test().await);
    Ok(())
}
