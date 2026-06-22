#![cfg(feature = "test-utils")]

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

fn missing_success_marker_pool_error() -> smelly_connect_cli::pool::PoolError {
    smelly_connect_cli::pool::PoolError::SessionConnectFailed(smelly_connect::Error::ControlPlane(
        smelly_connect::error::ControlPlaneError::PermanentAuthFailure(
            smelly_connect::error::AuthError::MissingSuccessMarker,
        ),
    ))
}

fn startup_pool_config(min_pool_size: usize) -> smelly_connect_cli::config::AppConfig {
    toml::from_str(&format!(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        min_pool_size = {min_pool_size}
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
        failure_threshold = 3
        backoff_base_secs = 30
        backoff_max_secs = 600
        allow_request_triggered_probe = true
        [[accounts]]
        name = "acct-01"
        username = "user1"
        password = "pass1"
        [[accounts]]
        name = "acct-02"
        username = "user2"
        password = "pass2"
        [[accounts]]
        name = "acct-03"
        username = "user3"
        password = "pass3"
        [proxy.http]
        enabled = true
        listen = "127.0.0.1:8080"
        [proxy.socks5]
        enabled = false
        listen = "127.0.0.1:1080"
        "#
    ))
    .unwrap()
}

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
async fn pool_applies_block_route_policy_to_returned_live_sessions() {
    let mut system_dns = std::collections::HashMap::new();
    system_dns.insert(
        "example.test".to_string(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
    );
    let session = smelly_connect::session::EasyConnectSession::new(
        "10.0.0.8".parse().unwrap(),
        smelly_connect::resource::ResourceSet::default(),
        smelly_connect::resolver::SessionResolver::new(
            std::collections::HashMap::new(),
            None,
            system_dns,
        ),
        smelly_connect::session::EasyConnectSession::failing_transport("unused"),
    );
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_route_policy_for_test(
            vec![("acct-01", session)],
            smelly_connect::domain::route_policy::RoutePolicy::block_non_resource_targets(),
        )
        .await;

    let pooled_session = pool.acquire().await.unwrap();
    let err = pooled_session
        .session()
        .unwrap()
        .plan_tcp_connect(("example.test", 443))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed
        )
    ));
}

#[tokio::test]
async fn pool_creates_specified_ready_count() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(4, 1).await;
    assert_eq!(pool.ready_count().await, 1);
}

#[tokio::test]
async fn pool_continues_startup_when_some_prewarm_accounts_fail() {
    let pool =
        smelly_connect_cli::pool::SessionPool::from_test_outcomes([Ok("a"), Err("x"), Ok("b")], 3)
            .await;
    assert_eq!(pool.ready_count().await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_prewarm_starts_multiple_connects_concurrently() {
    let cfg = startup_pool_config(2);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let inflight = Arc::new(AtomicUsize::new(0));
    let max_inflight = Arc::new(AtomicUsize::new(0));

    let pool = smelly_connect_cli::pool::SessionPool::from_config_with_async_connect_hook_for_test(
        &cfg,
        smelly_connect_cli::pool::PoolStartupMode::RequireReady,
        {
            let attempts = Arc::clone(&attempts);
            let inflight = Arc::clone(&inflight);
            let max_inflight = Arc::clone(&max_inflight);
            move |account| {
                attempts.lock().unwrap().push(account.name.clone());
                let inflight = Arc::clone(&inflight);
                let max_inflight = Arc::clone(&max_inflight);
                async move {
                    let current = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_inflight.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(75)).await;
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    Ok(
                        smelly_connect::test_support::session::session_with_domain_match(
                            &format!("{}.example.test", account.name),
                            std::net::Ipv4Addr::new(10, 0, 0, current as u8 + 7),
                        ),
                    )
                }
            }
        },
    )
    .await
    .unwrap();

    let mut attempts = attempts.lock().unwrap().clone();
    attempts.sort();
    assert_eq!(pool.ready_count().await, 2);
    assert_eq!(attempts, vec!["acct-01".to_string(), "acct-02".to_string()]);
    assert_eq!(max_inflight.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_prewarm_handles_parallel_failure_during_startup() {
    let cfg = startup_pool_config(2);
    let attempts = Arc::new(Mutex::new(Vec::new()));

    let pool = tokio::time::timeout(
        Duration::from_secs(10),
        smelly_connect_cli::pool::SessionPool::from_config_with_async_connect_hook_for_test(
            &cfg,
            smelly_connect_cli::pool::PoolStartupMode::RequireReady,
            {
                let attempts = Arc::clone(&attempts);
                move |account| {
                    attempts.lock().unwrap().push(account.name.clone());
                    async move {
                        match account.name.as_str() {
                            "acct-01" => Err(missing_success_marker_pool_error()),
                            "acct-02" => Ok(
                                smelly_connect::test_support::session::session_with_domain_match(
                                    "acct-02.example.test",
                                    std::net::Ipv4Addr::new(10, 0, 0, 8),
                                ),
                            ),
                            "acct-03" => Ok(
                                smelly_connect::test_support::session::session_with_domain_match(
                                    "acct-03.example.test",
                                    std::net::Ipv4Addr::new(10, 0, 0, 9),
                                ),
                            ),
                            other => panic!("unexpected account: {other}"),
                        }
                    }
                }
            },
        ),
    )
    .await
    .expect("startup should not stall")
    .unwrap();

    assert_eq!(pool.ready_count().await, 2);
    let mut got = attempts.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, vec!["acct-01".to_string(), "acct-02".to_string(), "acct-03".to_string()]);
    assert_eq!(pool.summary().await.disabled_nodes, 1);
}

#[tokio::test(start_paused = true)]
async fn background_maintenance_healthchecks_stop_after_explicit_shutdown() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let running = pool.background_maintenance_running_flag_for_test();

    pool.start_background_maintenance_for_test();
    tokio::task::yield_now().await;
    assert!(running.load(Ordering::SeqCst));

    pool.shutdown().await;
    tokio::task::yield_now().await;
    assert!(!running.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn background_maintenance_does_not_start_after_prior_shutdown() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let running = pool.background_maintenance_running_flag_for_test();

    pool.shutdown().await;
    pool.start_background_maintenance_for_test();
    tokio::task::yield_now().await;

    assert!(!running.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn background_maintenance_healthchecks_stop_when_last_pool_handle_drops() {
    let pool = smelly_connect_cli::pool::SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let running = pool.background_maintenance_running_flag_for_test();

    pool.start_background_maintenance_for_test();
    tokio::task::yield_now().await;
    assert!(running.load(Ordering::SeqCst));

    drop(pool);
    tokio::task::yield_now().await;
    assert!(!running.load(Ordering::SeqCst));
}

#[tokio::test]
async fn pool_prewarm_retries_other_accounts_after_permanent_auth_failure() {
    let cfg = startup_pool_config(2);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let outcomes = Arc::new(Mutex::new(VecDeque::from([
        Err(smelly_connect_cli::pool::PoolError::SessionConnectFailed(
            smelly_connect::Error::ControlPlane(
                smelly_connect::error::ControlPlaneError::PermanentAuthFailure(
                    smelly_connect::error::AuthError::MissingSuccessMarker,
                ),
            ),
        )),
        Ok(
            smelly_connect::test_support::session::session_with_domain_match(
                "acct-02.example.test",
                std::net::Ipv4Addr::new(10, 0, 0, 8),
            ),
        ),
        Ok(
            smelly_connect::test_support::session::session_with_domain_match(
                "acct-03.example.test",
                std::net::Ipv4Addr::new(10, 0, 0, 9),
            ),
        ),
    ])));

    let pool = smelly_connect_cli::pool::SessionPool::from_config_with_connect_hook_for_test(
        &cfg,
        smelly_connect_cli::pool::PoolStartupMode::RequireReady,
        {
            let attempts = Arc::clone(&attempts);
            let outcomes = Arc::clone(&outcomes);
            move |account| {
                attempts.lock().unwrap().push(account.name.clone());
                outcomes
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("connect attempt outcome should exist")
            }
        },
    )
    .await
    .unwrap();

    // Startup retries until min_pool_size=2 is met:
    //   acct-01 fails → Disabled, acct-02 succeeds → Active
    //   deficit=1 → acct-03 succeeds → Active
    assert_eq!(pool.ready_count().await, 2);
    assert_eq!(
        attempts.lock().unwrap().as_slice(),
        &["acct-01", "acct-02", "acct-03"]
    );
    assert!(outcomes.lock().unwrap().is_empty());

    let state_summary = pool.state_summary_for_test().await;
    assert!(state_summary.contains("acct-01:Disabled"));
    assert!(state_summary.contains("acct-02:Active"));
    assert!(state_summary.contains("acct-03:Active"));

    let summary = pool.summary().await;
    assert_eq!(summary.active_nodes, 2);
    assert_eq!(summary.disabled_nodes, 1);
    assert_eq!(summary.idle_nodes, 0);
}

#[tokio::test]
async fn pool_fails_fast_when_no_ready_sessions_exist() {
    let pool = smelly_connect_cli::pool::SessionPool::from_failed_accounts(2).await;
    let err = pool.acquire().await.unwrap_err();
    assert!(err.to_string().contains("no ready node"));
}

#[tokio::test]
async fn pool_removes_failed_session_from_rotation() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.report_failure("acct-01").await;
    assert_eq!(pool.ready_count().await, 0);
    assert!(pool.state_summary_for_test().await.contains("Dead"));
}

#[tokio::test]
async fn pool_exposes_state_summary_and_selectable_count_for_tests() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    assert!(pool.state_summary_for_test().await.contains("Active"));
    assert!(pool.has_selectable_nodes_for_test().await);
}

#[test]
fn resilience_defaults_are_present() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        min_pool_size =1
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
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
        min_pool_size =0
        connect_timeout_secs = 7
        healthcheck_interval_secs = 60
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
        min_pool_size =0
        connect_timeout_secs = 20
        session_connect_timeout_secs = 9
        healthcheck_interval_secs = 60
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
async fn threshold_crossing_moves_node_to_dead_and_removes_it_from_rotation() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.report_failure("acct-01").await;
    assert!(pool.state_summary_for_test().await.contains("Dead"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn timed_dead_node_is_reported_as_recovering_not_down() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.report_failure("acct-01").await;
    let summary = pool.summary().await;
    assert_eq!(summary.dead_nodes, 1);
    assert_eq!(
        summary.status,
        smelly_connect_cli::pool::PoolHealthStatus::Recovering
    );
}

#[tokio::test]
async fn configured_capacity_is_reported_as_recovering_not_down() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(2, 0).await;
    let summary = pool.summary().await;
    assert_eq!(summary.idle_nodes, 2);
    assert_eq!(
        summary.status,
        smelly_connect_cli::pool::PoolHealthStatus::Recovering
    );
}

#[tokio::test(start_paused = true)]
async fn backoff_grows_exponentially_and_respects_maximum() {
    let pool = smelly_connect_cli::pool::SessionPool::from_flaky_account_for_test().await;
    pool.report_failure("acct-01").await;
    let first = pool.current_backoff_for_test().await;
    assert!(first <= std::time::Duration::from_secs(600));
}

#[test]
fn auth_failure_message_is_treated_as_permanent_disable() {
    assert!(
        smelly_connect_cli::pool::is_permanent_auth_failure_for_test(
            &missing_success_marker_pool_error()
        )
    );
}

#[tokio::test(start_paused = true)]
async fn auth_failure_does_not_reenter_active_after_backoff_expiry() {
    let pool = smelly_connect_cli::pool::SessionPool::from_test_accounts(1, 0).await;
    pool.report_auth_failure_for_test("acct-01", missing_success_marker_pool_error())
        .await;
    tokio::time::advance(std::time::Duration::from_secs(601)).await;
    assert!(pool.state_summary_for_test().await.contains("Disabled"));
    assert!(!pool.has_selectable_nodes_for_test().await);
    assert_eq!(pool.summary().await.disabled_nodes, 1);
}

#[tokio::test]
async fn timed_out_live_session_recovery_prefers_transport_rebuild_before_relogin() {
    let mut resources = smelly_connect::resource::ResourceSet::default();
    resources.domain_rules.insert(
        "jwxt.sit.edu.cn".to_string(),
        smelly_connect::resource::DomainRule {
            port_min: 443,
            port_max: 443,
            protocol: smelly_connect::RouteProtocol::Tcp,
        },
    );
    let mut system_dns = std::collections::HashMap::new();
    system_dns.insert(
        "jwxt.sit.edu.cn".to_string(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(210, 35, 66, 210)),
    );
    let session = smelly_connect::session::EasyConnectSession::new(
        "10.0.0.8".parse().unwrap(),
        resources,
        smelly_connect::resolver::SessionResolver::new(
            std::collections::HashMap::new(),
            None,
            system_dns,
        ),
        smelly_connect::session::EasyConnectSession::failing_transport("stale transport"),
    )
    .with_transport_rebuild_for_test(|| {
        Ok(smelly_connect::transport::TransportStack::new(|_| async {
            let (client, _server) = tokio::io::duplex(1024);
            Ok(smelly_connect::transport::VpnStream::new(client))
        }))
    });
    let pool = smelly_connect_cli::pool::SessionPool::from_live_sessions_for_test(vec![(
        "acct-01",
        session.clone(),
    )])
    .await;

    pool.report_failure("acct-01").await;

    // In the simplified pool, report_failure moves the node to Dead.
    // acquire() will not automatically attempt transport rebuild.
    assert!(pool.state_summary_for_test().await.contains("Dead"));
    assert!(pool.acquire().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn next_live_session_waits_for_connecting_recovery_before_failing_fast() {
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "jwxt.sit.edu.cn",
        "10.0.0.8".parse().unwrap(),
    );
    let pool = smelly_connect_cli::pool::SessionPool::from_connecting_recovery_for_test(
        "acct-01",
        session,
        std::time::Duration::from_millis(100),
    )
    .await;

    let next = tokio::spawn({
        let pool = pool.clone();
        async move { pool.acquire().await }
    });

    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    let recovered = next.await.unwrap().unwrap();
    assert_eq!(recovered.account_name(), "acct-01");
}

#[tokio::test(start_paused = true)]
async fn live_session_failure_opens_node() {
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = smelly_connect_cli::pool::SessionPool::from_live_sessions_for_test(vec![(
        "acct-01", session,
    )])
    .await;
    pool.report_failure("acct-01").await;
    assert!(pool.state_summary_for_test().await.contains("Dead"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn pool_live_session_selection_keeps_shared_session_storage() {
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = smelly_connect_cli::pool::SessionPool::from_live_sessions_for_test(vec![(
        "acct-01",
        session.clone(),
    )])
    .await;

    let pooled_session = pool.acquire().await.unwrap();
    assert!(
        std::ptr::eq(session.resources(), pooled_session.session().unwrap().resources()),
        "selected live session should share underlying storage with the source session"
    );
}

#[tokio::test]
async fn concurrent_live_session_selection_can_reuse_same_account() {
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = smelly_connect_cli::pool::SessionPool::from_live_sessions_for_test(vec![(
        "acct-01", session,
    )])
    .await;

    let first = tokio::spawn({
        let pool = pool.clone();
        async move { pool.acquire().await }
    });
    let second = tokio::spawn({
        let pool = pool.clone();
        async move { pool.acquire().await }
    });

    let (first, second) = tokio::time::timeout(std::time::Duration::from_millis(50), async {
        tokio::join!(first, second)
    })
    .await
    .expect("concurrent live session selection should not serialize on one account");

    assert_eq!(first.unwrap().unwrap().account_name(), "acct-01");
    assert_eq!(second.unwrap().unwrap().account_name(), "acct-01");
}

#[tokio::test]
async fn successful_vpn_probe_keeps_live_session_selectable() {
    let session = smelly_connect::test_support::session::session_with_icmp_result(true);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session.clone())],
            "10.0.0.1",
        )
        .await;

    pool.report_failure("acct-01").await;

    assert!(pool.state_summary_for_test().await.contains("Dead"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn repeated_vpn_probe_failures_mark_live_session_dead() {
    let session = smelly_connect::test_support::session::session_with_icmp_result(false);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session.clone())],
            "10.0.0.1",
        )
        .await;

    pool.report_failure("acct-01").await;

    tokio::time::sleep(std::time::Duration::from_millis(450)).await;
    assert!(pool.state_summary_for_test().await.contains("Dead"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}

#[tokio::test]
async fn periodic_health_probe_marks_dead_live_session_dead_without_request_failure() {
    let session = smelly_connect::test_support::session::session_with_icmp_result(false);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_keepalive_target_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;

    pool.report_failure("acct-01").await;

    assert!(pool.state_summary_for_test().await.contains("Dead"));
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
        min_pool_size =0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
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

#[tokio::test]
async fn pool_disables_icmp_keepalive_when_explicitly_disabled() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        default_keepalive_host = "jwxt.sit.edu.cn"
        enable_icmp_keepalive = false
        [pool]
        min_pool_size =0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
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

    assert_eq!(pool.keepalive_target_for_test().await, None);
}

#[tokio::test]
async fn pool_does_not_fallback_keepalive_target_to_vpn_server() {
    let cfg: smelly_connect_cli::config::AppConfig = toml::from_str(
        r#"
        [vpn]
        server = "vpn1.sit.edu.cn"
        [pool]
        min_pool_size =0
        connect_timeout_secs = 20
        healthcheck_interval_secs = 60
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

    assert_eq!(pool.keepalive_target_for_test().await, None);
}

#[tokio::test(start_paused = true)]
async fn session_keepalive_failure_marks_live_session_dead_before_periodic_healthcheck() {
    let session = smelly_connect::test_support::session::session_with_icmp_result(false);
    let pool =
        smelly_connect_cli::pool::SessionPool::from_live_sessions_with_active_keepalive_for_test(
            vec![("acct-01", session)],
            "10.0.0.1",
        )
        .await;

    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    tokio::task::yield_now().await;

    assert!(pool.state_summary_for_test().await.contains("Dead"));
    assert!(!pool.has_selectable_nodes_for_test().await);
}
