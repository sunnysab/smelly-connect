use smelly_connect::kernel::control::{
    parse_conf_metadata, parse_login_auth_challenge, parse_login_success, parse_resource_document,
};

#[test]
fn login_auth_challenge_extracts_required_fields() {
    let body = include_str!("fixtures/login_auth_requires_captcha.xml");
    let parsed = parse_login_auth_challenge(body).unwrap();
    assert_eq!(parsed.twfid, "dummy-twfid");
    assert!(parsed.requires_captcha);
    assert_eq!(parsed.legacy_cipher_hint.as_deref(), Some("RC4-SHA"));
}

#[test]
fn login_success_preserves_twfid_fallback_behavior() {
    let body = "<Response><Result>1</Result></Response>";
    let twfid = parse_login_success(body, "previous-twfid").unwrap();
    assert_eq!(twfid, "previous-twfid");
}

#[test]
fn resource_document_reuses_existing_resource_shape() {
    let body = include_str!("fixtures/resource_sample.xml");
    let parsed = parse_resource_document(body).unwrap();
    assert!(parsed.domain_rules.contains_key("zju.edu.cn"));
    assert!(!parsed.ip_rules.is_empty());
}

#[test]
fn conf_metadata_parses_other_and_service() {
    let body = r#"
<Conf>
  <Other login_name="testuser" isRelogin="0" />
  <Service SvpnID="abc123" />
  <Mline enable="1" list="vpn1.example.com:443;vpn2.example.com:443" />
</Conf>
"#;
    let meta = parse_conf_metadata(body).unwrap();
    assert_eq!(meta.login_name.as_deref(), Some("testuser"));
    assert_eq!(meta.is_relogin.as_deref(), Some("0"));
    assert_eq!(meta.svpn_id.as_deref(), Some("abc123"));
    assert!(meta.mline_enable);
    assert_eq!(meta.mline_list.len(), 2);
    assert_eq!(meta.mline_list[0], "vpn1.example.com:443");
}

#[test]
fn conf_metadata_defaults_when_absent() {
    let body = "<Conf></Conf>";
    let meta = parse_conf_metadata(body).unwrap();
    assert!(meta.login_name.is_none());
    assert!(meta.is_relogin.is_none());
    assert!(meta.svpn_id.is_none());
    assert!(!meta.mline_enable);
    assert!(meta.mline_list.is_empty());
}
