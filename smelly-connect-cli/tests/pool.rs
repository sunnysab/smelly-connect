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
    pool.force_failures_for_test(3).await;
    assert_eq!(pool.ready_count().await, 0);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(pool.ready_count().await >= 1);
}

#[tokio::test]
async fn pool_exposes_state_summary_and_selectable_count_for_tests() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    assert!(pool.state_summary_for_test().await.contains("Ready"));
    assert!(pool.has_selectable_nodes_for_test().await);
}

#[test]
fn resilience_defaults_are_present() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        "#,
    )
    .unwrap();
    assert_eq!(cfg.pool.failure_threshold, 3);
    assert_eq!(cfg.pool.backoff_base_secs, 30);
    assert_eq!(cfg.pool.backoff_max_secs, 600);
    assert!(cfg.pool.allow_request_triggered_probe);
}

#[tokio::test]
async fn single_failure_marks_node_suspect_but_keeps_it_selectable() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(1).await;
    assert!(pool.state_summary_for_test().await.contains("Suspect"));
    assert!(pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn threshold_crossing_moves_node_to_open_and_removes_it_from_rotation() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    assert!(pool.state_summary_for_test().await.contains("Open"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn normal_selection_uses_ready_and_suspect_but_excludes_open_and_half_open() {
    let pool = smelly_connect_cli::pool::SessionPool::from_mixed_state_pool_for_test().await;
    let picks = pool.collect_selected_accounts_for_test(4).await;
    assert!(
        picks.iter()
            .all(|name| name == "ready-01" || name == "suspect-01")
    );
}
