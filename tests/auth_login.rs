use smelly_connect::{EasyConnectConfig, TargetAddr};

#[test]
fn public_api_smoke_compiles() {
    let _cfg = EasyConnectConfig::new("rvpn.example.com", "user", "pass");
    let _target = TargetAddr::from(("libdb.zju.edu.cn", 443));
}

#[tokio::test]
async fn login_auth_parses_captcha_requirement() {
    let body = include_str!("fixtures/login_auth_requires_captcha.xml");
    let parsed = smelly_connect::auth::parse_login_auth(body).unwrap();
    assert!(parsed.requires_captcha);
    assert_eq!(parsed.twfid, "dummy-twfid");
}

#[tokio::test]
async fn captcha_callback_receives_image_bytes() {
    let handler = smelly_connect::CaptchaHandler::from_async(|bytes, mime| async move {
        assert!(!bytes.is_empty());
        assert_eq!(mime.as_deref(), Some("image/jpeg"));
        Ok::<_, smelly_connect::CaptchaError>("1234".to_string())
    });
    let value = handler
        .solve(vec![1, 2, 3], Some("image/jpeg".to_string()))
        .await
        .unwrap();
    assert_eq!(value, "1234");
}

#[test]
fn login_password_payload_uses_rsa_and_optional_csrf_suffix() {
    let modulus = "DD404D684D4830F755E0EA3004C933395BAC82AD43687483A15DDFA7A07F4AD94216E90AECECFCF1D905155EC64B4516259681DBC0F76A292091D840AE09AAA9EC4870629D1B7F1971E82094AEAC634D72E27E9C164744EA35C881D802D6647F4BE90B90FC9C84E80C3ADC1B09223018B9A8ACDD22C63469C5896B18BE6B169B";
    let encrypted =
        smelly_connect::auth::encrypt_password("pass", Some("csrf"), modulus, 65537).unwrap();
    assert!(!encrypted.is_empty());
}

#[test]
fn login_psw_success_updates_twfid() {
    let body = include_str!("fixtures/login_psw_success.xml");
    let twfid = smelly_connect::protocol::parse_login_psw_success(body).unwrap();
    assert_eq!(twfid, "updated-twfid");
}

#[test]
fn assigned_ip_reply_extracts_ipv4() {
    let reply = [0x00, 0x00, 0x00, 0x00, 10, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    let ip = smelly_connect::protocol::parse_assigned_ip_reply(&reply).unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
}
