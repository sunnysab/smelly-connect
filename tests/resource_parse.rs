#[test]
fn parses_domain_and_ip_resources() {
    let body = include_str!("fixtures/resource_sample.xml");
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.domain_rules.contains_key("zju.edu.cn"));
    assert!(!parsed.ip_rules.is_empty());
    assert_eq!(
        parsed.static_dns.get("libdb.zju.edu.cn").unwrap().to_string(),
        "10.0.0.8"
    );
}
