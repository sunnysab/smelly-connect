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
async fn pool_uses_connect_timeout_secs_for_recovery_login_timeout() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 0
        connect_timeout_secs = 7
        healthcheck_interval_secs = 60
        selection = "round_robin"
        failure_threshold = 3
        backoff_base_secs = 30
        backoff_max_secs = 600
        allow_request_triggered_probe = true
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

    let pool = smelly_connect_cli::pool::SessionPool::from_config_allow_empty(&cfg)
        .await
        .unwrap();
    assert_eq!(
        pool.connect_timeout_for_test().await,
        std::time::Duration::from_secs(7)
    );
}

#[tokio::test]
async fn pool_prefers_session_connect_timeout_secs_over_legacy_timeout() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        session_connect_timeout_secs = 9
        healthcheck_interval_secs = 60
        selection = "round_robin"
        failure_threshold = 3
        backoff_base_secs = 30
        backoff_max_secs = 600
        allow_request_triggered_probe = true
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

    let pool = smelly_connect_cli::pool::SessionPool::from_config_allow_empty(&cfg)
        .await
        .unwrap();
    assert_eq!(
        pool.connect_timeout_for_test().await,
        std::time::Duration::from_secs(9)
    );
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
async fn timed_open_node_is_reported_as_recovering_not_down() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    let summary = pool.summary().await;
    assert_eq!(summary.open_nodes, 1);
    assert_eq!(
        summary.status,
        smelly_connect_cli::pool::PoolHealthStatus::Recovering
    );
}

#[tokio::test]
async fn configured_capacity_is_reported_as_recovering_not_down() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(2, 0).await;
    let summary = pool.summary().await;
    assert_eq!(summary.configured_nodes, 2);
    assert_eq!(
        summary.status,
        smelly_connect_cli::pool::PoolHealthStatus::Recovering
    );
}

#[tokio::test]
async fn normal_selection_uses_ready_and_suspect_but_excludes_open_and_half_open() {
    let pool = smelly_connect_cli::pool::SessionPool::from_mixed_state_pool_for_test().await;
    let picks = pool.collect_selected_accounts_for_test(4).await;
    assert!(
        picks
            .iter()
            .all(|name| name == "ready-01" || name == "suspect-01")
    );
}

#[tokio::test(start_paused = true)]
async fn backoff_grows_exponentially_and_respects_maximum() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    let first = pool.current_backoff_for_test().await;
    pool.force_probe_failure_for_test().await;
    let second = pool.current_backoff_for_test().await;
    assert!(second > first);
    assert!(second <= std::time::Duration::from_secs(600));
}

#[tokio::test(start_paused = true)]
async fn open_node_reenters_via_timer_into_half_open_after_backoff_expiry() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.force_failures_for_test(3).await;
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    assert!(pool.state_summary_for_test().await.contains("HalfOpen"));
}

#[tokio::test(start_paused = true)]
async fn request_triggered_probe_recovers_one_node_when_pool_is_exhausted() {
    let pool = smelly_connect_cli::pool::SessionPool::from_exhausted_pool_for_test().await;
    let err = pool
        .try_request_triggered_probe_for_test()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no ready session"));
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let recovered = pool.try_request_triggered_probe_for_test().await.unwrap();
    assert_eq!(recovered.account_name(), "acct-01");
}

#[tokio::test(start_paused = true)]
async fn concurrent_requests_do_not_probe_same_node_twice() {
    let pool = smelly_connect_cli::pool::SessionPool::from_exhausted_pool_for_test().await;
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let results = pool.run_concurrent_probe_race_for_test().await;
    assert_eq!(results.successes, 1);
    assert_eq!(results.fast_failures, 1);
}

#[tokio::test(start_paused = true)]
async fn successful_probe_returns_node_to_ready_and_back_into_normal_rotation() {
    let pool = smelly_connect_cli::pool::SessionPool::from_exhausted_pool_for_test().await;
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let _ = pool.try_request_triggered_probe_for_test().await.unwrap();
    assert!(pool.has_selectable_nodes_for_test().await);
    let picks = pool.collect_selected_accounts_for_test(1).await;
    assert_eq!(picks, vec!["acct-01".to_string()]);
}

#[tokio::test(start_paused = true)]
async fn live_session_failure_opens_node_and_request_triggered_probe_can_recover() {
    let session = smelly_connect::session::tests::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = smelly_connect_cli::pool::SessionPool::from_live_sessions_for_test(vec![(
        "acct-01", session,
    )])
    .await;
    pool.report_live_session_failure("acct-01", "forced live connect failure")
        .await;
    assert!(pool.state_summary_for_test().await.contains("Open"));
    assert!(!pool.has_selectable_nodes_for_test().await);

    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    let recovered = pool.try_request_triggered_probe_for_test().await.unwrap();
    assert_eq!(recovered.account_name(), "acct-01");
}

#[tokio::test]
async fn successful_vpn_probe_keeps_live_session_selectable() {
    let session = smelly_connect::session::tests::session_with_icmp_result(true);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session.clone())],
            "10.0.0.1",
        )
        .await;

    pool.report_live_session_unhealthy_if_probe_fails("acct-01", &session, "forced target failure")
        .await;

    assert!(pool.state_summary_for_test().await.contains("Ready"));
    assert!(pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn repeated_vpn_probe_failures_mark_live_session_open() {
    let session = smelly_connect::session::tests::session_with_icmp_result(false);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session.clone())],
            "10.0.0.1",
        )
        .await;

    pool.report_live_session_unhealthy_if_probe_fails("acct-01", &session, "forced target failure")
        .await;

    tokio::time::sleep(std::time::Duration::from_millis(450)).await;
    assert!(pool.state_summary_for_test().await.contains("Open"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn concurrent_live_session_failures_share_one_vpn_probe() {
    let probe_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let session = smelly_connect::session::tests::session_with_delayed_icmp_result(
        false,
        std::time::Duration::from_millis(50),
        probe_count.clone(),
    );
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session.clone())],
            "vpn1.sit.edu.cn",
        )
        .await;

    let first = pool.report_live_session_unhealthy_if_probe_fails(
        "acct-01",
        &session,
        "forced target failure",
    );
    let second = pool.report_live_session_unhealthy_if_probe_fails(
        "acct-01",
        &session,
        "forced target failure",
    );
    tokio::join!(first, second);

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(probe_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn periodic_health_probe_marks_dead_live_session_open_without_request_failure() {
    let session = smelly_connect::session::tests::session_with_icmp_result(false);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;

    pool.run_periodic_healthcheck_once_for_test().await;

    assert!(pool.state_summary_for_test().await.contains("Open"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn pool_prefers_default_keepalive_host_over_vpn_server() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        default_keepalive_host = "jwxt.sit.edu.cn"
        [pool]
        prewarm = 0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        selection = "round_robin"
        failure_threshold = 3
        backoff_base_secs = 30
        backoff_max_secs = 600
        allow_request_triggered_probe = true
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

    let pool = smelly_connect_cli::pool::SessionPool::from_config_allow_empty(&cfg)
        .await
        .unwrap();

    assert_eq!(
        pool.keepalive_target_for_test().await.as_deref(),
        Some("jwxt.sit.edu.cn")
    );
}

#[tokio::test(start_paused = true)]
async fn session_keepalive_failure_marks_live_session_open_before_periodic_healthcheck() {
    let session = smelly_connect::session::tests::session_with_icmp_result(false);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_active_keepalive_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;

    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    tokio::task::yield_now().await;

    assert!(pool.state_summary_for_test().await.contains("Open"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}
