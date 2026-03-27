use smelly_connect::{CaptchaError, CaptchaHandler, EasyConnectConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server = std::env::var("VPN_HOST")
        .ok()
        .or_else(|| std::env::var("VPN_URL").ok())
        .map(normalize_server)
        .expect("VPN_HOST or VPN_URL");
    let username = std::env::var("VPN_USER").expect("VPN_USER");
    let password = std::env::var("VPN_PASS").expect("VPN_PASS");
    let host = std::env::var("TARGET_HOST").unwrap_or_else(|_| "172.24.9.11".to_string());
    let port = std::env::var("TARGET_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(80);
    let path = std::env::var("TARGET_PATH").unwrap_or_else(|_| "/index.html".to_string());

    let config = EasyConnectConfig::new(server, username, password).with_captcha_handler(
        CaptchaHandler::from_async(|_, _| async move {
            Err(CaptchaError::new(
                "captcha callback not expected for this server",
            ))
        }),
    );
    let session = config.connect().await.expect("connect session");
    let mut stream = session
        .connect_tcp((host.as_str(), port))
        .await
        .expect("connect tcp");

    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut total = 0usize;
    let mut first = true;
    loop {
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(0)) => {
                println!("EOF total={total}");
                break;
            }
            Ok(Ok(n)) => {
                total += n;
                println!("chunk={n} total={total}");
                if first {
                    let preview = String::from_utf8_lossy(&buf[..n.min(512)]);
                    println!("preview:\n{preview}");
                    first = false;
                }
            }
            Ok(Err(err)) => {
                eprintln!("read error total={total} err={err}");
                break;
            }
            Err(_) => {
                eprintln!("read timeout total={total}");
                break;
            }
        }
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
