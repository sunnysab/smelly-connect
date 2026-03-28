use smelly_connect::{CaptchaError, CaptchaHandler, EasyConnectConfig, run_control_plane};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server = std::env::var("VPN_HOST")
        .ok()
        .or_else(|| std::env::var("VPN_URL").ok())
        .map(normalize_server)
        .expect("VPN_HOST or VPN_URL");
    let username = std::env::var("VPN_USER").expect("VPN_USER");
    let password = std::env::var("VPN_PASS").expect("VPN_PASS");
    let host = std::env::var("TARGET_HOST").unwrap_or_else(|_| "jwxt.sit.edu.cn".to_string());
    let port = std::env::var("TARGET_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(443);

    let config = EasyConnectConfig::new(server, username, password).with_captcha_handler(
        CaptchaHandler::from_async(|_, _| async move {
            Err(CaptchaError::new(
                "captcha callback not expected for this server",
            ))
        }),
    );
    let state = run_control_plane(&config).await.expect("control plane");

    println!("domain rules: {}", state.resources.domain_rules.len());
    println!("ip rules: {}", state.resources.ip_rules.len());
    println!("static dns: {}", state.resources.static_dns.len());
    println!("remote dns server: {:?}", state.resources.remote_dns_server);

    let mut matched = Vec::new();
    for domain in state.resources.domain_rules.keys() {
        let ok = if domain.starts_with('.') {
            host.ends_with(domain)
        } else {
            host == *domain || host.ends_with(&format!(".{domain}"))
        };
        if ok {
            matched.push(domain.clone());
        }
    }
    matched.sort();

    println!("matched domains: {}", matched.len());
    for domain in matched.iter().take(20) {
        println!("match: {domain}");
    }

    let resolved = tokio::net::lookup_host((host.as_str(), port))
        .await
        .expect("system lookup");
    for addr in resolved {
        let allowed = state
            .resources
            .matches_ip(addr.ip(), port, smelly_connect::RouteProtocol::Tcp);
        println!("resolved: {addr} allowed={allowed}");
    }
}

fn normalize_server(value: String) -> String {
    value
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}
