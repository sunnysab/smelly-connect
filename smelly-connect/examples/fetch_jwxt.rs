use std::time::{Duration, Instant};

use smelly_connect::{CaptchaError, CaptchaHandler, EasyConnectConfig};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server = std::env::var("VPN_HOST")
        .ok()
        .or_else(|| std::env::var("VPN_URL").ok())
        .map(normalize_server)
        .expect("VPN_HOST or VPN_URL");
    let username = std::env::var("VPN_USER").expect("VPN_USER");
    let password = std::env::var("VPN_PASS").expect("VPN_PASS");
    let target =
        std::env::var("TARGET_URL").unwrap_or_else(|_| "https://jwxt.sit.edu.cn/".to_string());
    let hold_seconds = std::env::var("HOLD_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let idle_mode = std::env::var("IDLE_MODE")
        .ok()
        .map(|value| value == "1")
        .unwrap_or(false);
    let smoke_tcp = std::env::var("SMOKE_TCP")
        .ok()
        .map(|value| value == "1")
        .unwrap_or(false);
    let smoke_icmp = std::env::var("SMOKE_ICMP")
        .ok()
        .map(|value| value == "1")
        .unwrap_or(false);

    let mut config = EasyConnectConfig::new(server, username, password).with_captcha_handler(
        CaptchaHandler::from_async(|_, _| async move {
            Err(CaptchaError::new(
                "captcha callback not expected for this server",
            ))
        }),
    );
    if let Ok(target) = std::env::var("KEEPALIVE_ICMP_TARGET") {
        config = config
            .with_icmp_keepalive(target)
            .with_icmp_keepalive_interval(Duration::from_secs(60));
    }

    let session = config.connect().await.expect("vpn connect");
    println!("client ip: {}", session.client_ip());

    if smoke_tcp {
        let url = reqwest::Url::parse(&target).expect("parse target url");
        let host = url.host_str().expect("target host");
        let port = url.port_or_known_default().expect("target port");
        let _stream = session
            .connect_tcp((host, port))
            .await
            .expect("tcp connect");
        println!("tcp connect ok: {host}:{port}");
        return;
    }

    if smoke_icmp {
        let url = reqwest::Url::parse(&target).expect("parse target url");
        let host = url.host_str().expect("target host");
        session.icmp_ping(host.into()).await.expect("icmp ping");
        println!("icmp ping ok: {host}");
        return;
    }

    let client = session.reqwest_client().await.expect("reqwest client");

    let first = fetch_once(&client, &target).await;
    println!("first fetch: {}", first);
    println!("jwxt ok");

    if hold_seconds == 0 {
        return;
    }

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(hold_seconds) {
        tokio::time::sleep(Duration::from_secs(30)).await;
        if !idle_mode {
            let attempt = fetch_once(&client, &target).await;
            println!("hold fetch: {}", attempt);
        }
    }

    if idle_mode {
        let attempt = fetch_once(&client, &target).await;
        println!("idle fetch: {}", attempt);
    }

    println!("hold complete: {}s", hold_seconds);
    println!("hold ok");
}

async fn fetch_once(client: &reqwest::Client, target: &str) -> String {
    let response = client.get(target).send().await.expect("send request");
    let status = response.status();
    let body = response.text().await.expect("read body");
    let body_len = body.len();
    let has_html = body.to_ascii_lowercase().contains("<html");
    format!("status={status} body_len={body_len} html={has_html}")
}

fn normalize_server(value: String) -> String {
    value
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}
