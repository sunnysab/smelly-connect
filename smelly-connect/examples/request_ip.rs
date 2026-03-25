use smelly_connect::auth::control::{request_ip_for_server, request_token, run_control_plane};
use smelly_connect::{CaptchaHandler, EasyConnectConfig};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server = std::env::var("VPN_HOST").expect("VPN_HOST");
    let username = std::env::var("VPN_USER").expect("VPN_USER");
    let password = std::env::var("VPN_PASS").expect("VPN_PASS");

    let config = EasyConnectConfig::new(server.clone(), username, password).with_captcha_handler(
        CaptchaHandler::from_async(|_, _| async move {
            Err(smelly_connect::CaptchaError::new(
                "captcha callback not expected for this server",
            ))
        }),
    );

    let state = run_control_plane(&config).await.expect("control plane");
    let token = request_token(&format!("{server}:443"), &state.authorized_twfid).expect("token");
    let ip = request_ip_for_server(&server, &token, state.legacy_cipher_hint.as_deref())
        .await
        .expect("request ip");

    println!("legacy cipher hint: {:?}", state.legacy_cipher_hint);
    println!("client ip: {ip}");
}
