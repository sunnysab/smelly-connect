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
fn parses_sslctx_from_other_element() {
    let body = r#"
<Resource>
  <Other sslctx="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeffaabbccdd00112233aabbccdd0011223344556677889900aa44556677889900aa" defaultRcId="1" />
  <Rcs>
    <Rc type="1" proto="-1" host="10.0.0.1" port="443~443" />
  </Rcs>
  <Dns data="" dnsserver="10.10.0.21" />
</Resource>
"#;
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.sslctx.is_some());
    let hex_str = parsed.sslctx.unwrap();
    assert_eq!(hex_str.len(), 128);
    let decoded = smelly_connect::kernel::tunnel::decode_sslctx_hex(&hex_str).unwrap();
    assert_eq!(decoded.key().len(), 16);
    assert_eq!(decoded.randnum().len(), 16);
}

#[test]
fn sslctx_absent_when_no_other_element() {
    let body = include_str!("fixtures/resource_sample.xml");
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.sslctx.is_none());
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
    assert!(parsed.matches_domain("jwxt.sit.edu.cn", 443, smelly_connect::RouteProtocol::Tcp,));
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
    assert!(
        !parsed
            .domain_rules
            .contains_key("myportal.sit.edu.cn?rnd=1")
    );
}

#[test]
fn static_dns_parses_ipv6_targets_without_truncation() {
    let body = r#"
<Resource>
  <Rcs>
    <Rc type="1" proto="-1" host="jwxt.sit.edu.cn" port="443~443" />
  </Rcs>
  <Dns data="1:ipv6.example.edu:2600:10:20::40" dnsserver="10.10.0.21" />
</Resource>
"#;
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert_eq!(
        parsed
            .static_dns
            .get("ipv6.example.edu")
            .unwrap()
            .to_string(),
        "2600:10:20::40"
    );
}

#[test]
fn single_ipv6_resource_is_parsed_as_ip_rule_not_domain_rule() {
    let body = r#"
<Resource>
  <Rcs>
    <Rc type="1" proto="-1" host="2600:1417:9800::45c0:da78" port="443~443" />
  </Rcs>
  <Dns data="" dnsserver="10.10.0.21" />
</Resource>
"#;
    let parsed = smelly_connect::resource::parse_resources(body).unwrap();
    assert!(parsed.domain_rules.is_empty());
    assert_eq!(parsed.ip_rules.len(), 1);
    assert_eq!(
        parsed.ip_rules[0].ip_min.to_string(),
        "2600:1417:9800::45c0:da78"
    );
    assert_eq!(
        parsed.ip_rules[0].ip_max.to_string(),
        "2600:1417:9800::45c0:da78"
    );
}
