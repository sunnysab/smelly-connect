#[tokio::test]
async fn pool_prewarms_first_n_accounts() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(4, 2).await;
    assert_eq!(pool.ready_count().await, 2);
}

#[tokio::test]
async fn pool_selects_ready_sessions_round_robin() {
    let pool =
        smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["a", "b", "c"]).await;
    assert_eq!(pool.next_account_name().await.unwrap(), "a");
    assert_eq!(pool.next_account_name().await.unwrap(), "b");
    assert_eq!(pool.next_account_name().await.unwrap(), "c");
    assert_eq!(pool.next_account_name().await.unwrap(), "a");
}

#[tokio::test]
async fn pool_lazily_connects_remaining_accounts_on_demand() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(4, 1).await;
    pool.ensure_additional_capacity_for_test().await.unwrap();
    assert!(pool.ready_count().await >= 2);
}

#[tokio::test]
async fn pool_continues_startup_when_some_prewarm_accounts_fail() {
    let pool =
        smelly_connect_cli::pool::SessionPool::from_test_outcomes([Ok("a"), Err("x"), Ok("b")], 3)
            .await;
    assert_eq!(pool.ready_count().await, 2);
}

#[tokio::test]
async fn pool_fails_fast_when_no_ready_sessions_exist() {
    let pool = smelly_connect_cli::pool::SessionPool::from_failed_accounts(2).await;
    let err = pool.next_session().await.unwrap_err();
    assert!(err.to_string().contains("no ready session"));
}

#[tokio::test]
async fn pool_removes_failed_session_from_rotation_and_retries_after_fixed_delay() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_one_failure_for_test().await;
    assert_eq!(pool.ready_count().await, 0);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(pool.ready_count().await >= 1);
}
