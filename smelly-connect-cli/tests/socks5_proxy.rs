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
async fn socks5_proxy_returns_failure_when_no_ready_session_exists() {
    let result = smelly_connect_cli::proxy::socks5::proxy_socks5_no_ready_session_for_test()
        .await
        .unwrap();
    assert_eq!(result.reply_code, 0x01);
}
