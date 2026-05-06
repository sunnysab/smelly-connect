use smelly_connect::kernel::control::{
    parse_login_auth_challenge, parse_login_success, parse_resource_document,
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
