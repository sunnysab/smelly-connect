#[test]
fn parses_domain_and_ip_resources() {
    let body = include_str!("fixtures/resource_sample.xml");
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.domain_rules.contains_key("zju.edu.cn"));
    assert!(!parsed.ip_rules.is_empty());
    assert_eq!(
        parsed
            .static_dns
            .get("libdb.zju.edu.cn")
            .unwrap()
            .to_string(),
        "10.0.0.8"
    );
}

#[test]
fn wildcard_domain_rules_match_subdomains() {
    let body = r#"
<Resource>
  <Rcs>
    <Rc type="1" proto="-1" host="*.sit.edu.cn" port="443~443" />
  </Rcs>
  <Dns data="" dnsserver="10.10.0.21" />
</Resource>
"#;
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.domain_rules.contains_key(".sit.edu.cn"));
    assert!(parsed.matches_domain("jwxt.sit.edu.cn", 443));
}

#[test]
fn domain_rules_strip_port_and_query_suffixes() {
    let body = r#"
<Resource>
  <Rcs>
    <Rc type="1" proto="-1" host="app1.sit.edu.cn:81;myportal.sit.edu.cn?rnd=1" port="80~80;443~443" />
  </Rcs>
  <Dns data="" dnsserver="10.10.0.21" />
</Resource>
"#;
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.domain_rules.contains_key("app1.sit.edu.cn"));
    assert!(parsed.domain_rules.contains_key("myportal.sit.edu.cn"));
    assert!(!parsed.domain_rules.contains_key("app1.sit.edu.cn:81"));
    assert!(!parsed.domain_rules.contains_key("myportal.sit.edu.cn?rnd=1"));
}
