#[tokio::test]
async fn resolver_falls_back_from_remote_dns_to_system_dns() {
    let resolver = smelly_connect::resolver::tests::resolver_with_failing_remote();
    let ip = resolver.resolve_for_vpn("libdb.zju.edu.cn").await.unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
}

#[tokio::test]
async fn resolver_uses_system_lookup_when_no_cached_record_exists() {
    let resolver = smelly_connect::resolver::SessionResolver::new(
        std::collections::HashMap::new(),
        None,
        std::collections::HashMap::new(),
    );
    let ip = resolver.resolve_for_vpn("localhost").await.unwrap();
    assert_eq!(ip.to_string(), "127.0.0.1");
}
