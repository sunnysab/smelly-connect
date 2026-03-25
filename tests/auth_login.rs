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
