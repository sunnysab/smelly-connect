#[tokio::test]
async fn resolver_falls_back_from_remote_dns_to_system_dns() {
    let resolver = smelly_connect::resolver::tests::resolver_with_failing_remote();
    let ip = resolver.resolve_for_vpn("libdb.zju.edu.cn").await.unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
}
