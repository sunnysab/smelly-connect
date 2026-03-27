#[tokio::test]
async fn socks5_proxy_supports_tcp_connect() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_for_test()
        .await
        .unwrap();
    assert_eq!(result.account_name, "acct-01");
    assert!(result.used_pool_selection);
    assert_eq!(result.echoed_bytes, b"ping");
}

#[tokio::test]
async fn socks5_proxy_supports_ipv6_targets() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_ipv6_for_test()
        .await
        .unwrap();
    assert_eq!(result.account_name, "acct-01");
    assert!(result.used_pool_selection);
    assert_eq!(result.echoed_bytes, b"ping");
}

#[tokio::test]
async fn socks5_proxy_supports_udp_associate() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_udp_associate_for_test()
        .await
        .unwrap();
    assert_eq!(result.account_name, "acct-01");
    assert!(result.used_pool_selection);
    assert_eq!(result.echoed_bytes, b"ping");
}

#[tokio::test]
async fn socks5_proxy_returns_failure_when_no_ready_session_exists() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_no_ready_session_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x03);
}

#[tokio::test]
async fn socks5_proxy_listener_stays_bound_during_total_pool_outage() {
    let results =
        smelly_connect_cli::proxy::socks5::proxy_socks5_no_ready_session_sequence_for_test(2)
            .await
            .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.reply_code == 0x03));
}

#[tokio::test]
async fn socks5_proxy_updates_runtime_stats_after_tunneling() {
    let snapshot = smelly_connect_cli::proxy::socks5::proxy_socks5_runtime_stats_for_test()
        .await
        .unwrap();
    assert_eq!(snapshot.socks5.current_connections, 0);
    assert_eq!(snapshot.socks5.total_connections, 1);
    assert!(snapshot.socks5.client_to_upstream_bytes >= 4);
    assert!(snapshot.socks5.upstream_to_client_bytes >= 4);
}

#[tokio::test]
async fn socks5_proxy_enforces_connect_timeout() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_connect_timeout_for_test()
        .await
        .unwrap();
    assert!(result.elapsed < std::time::Duration::from_millis(150));
}

#[tokio::test]
async fn socks5_proxy_returns_failure_reply_on_upstream_connect_error() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_connect_failure_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x03);
}

#[tokio::test]
async fn socks5_proxy_returns_failure_reply_on_upstream_timeout() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_timeout_reply_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x03);
}

#[tokio::test]
async fn socks5_proxy_rejects_unsupported_auth_methods() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_rejects_unsupported_methods_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0xff);
}

#[tokio::test]
async fn socks5_proxy_rejects_unsupported_commands_with_reply() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_rejects_unsupported_command_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x07);
}

#[tokio::test]
async fn socks5_proxy_rejects_unsupported_address_types_with_reply() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_rejects_unsupported_atyp_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x08);
}

#[tokio::test]
async fn socks5_allow_all_connect_failure_does_not_mark_live_session_open() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_allow_all_failure_does_not_open_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x03);
    assert!(result.state_summary.contains("Ready"));
    assert!(result.selectable_after_failure);
}
