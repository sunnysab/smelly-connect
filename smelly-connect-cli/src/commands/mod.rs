pub mod inspect;
pub mod proxy;
pub mod routes;
pub mod status;
pub mod test;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::CliError;

async fn fetch_management_json<T>(target: &str, path: &str) -> Result<T, CliError>
where
    T: DeserializeOwned,
{
    let target = normalize_connect_target(target);
    let mut client = TcpStream::connect(&target)
        .await
        .map_err(|err| CliError::Command(format!("management connect failed: {err}")))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    client
        .write_all(request.as_bytes())
        .await
        .map_err(|err| CliError::Command(format!("management request failed: {err}")))?;
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| CliError::Command(format!("management read failed: {err}")))?;
    let response = String::from_utf8(response).map_err(|err| CliError::Command(err.to_string()))?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| CliError::Command("invalid management response".to_string()))?;
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| CliError::Command("missing management status line".to_string()))?;
    if !status_line.contains(" 200 ") {
        return Err(CliError::Command(format!(
            "management request failed: {status_line}"
        )));
    }
    serde_json::from_str(body)
        .map_err(|err| CliError::Command(format!("invalid management json: {err}")))
}

fn normalize_connect_target(target: &str) -> String {
    let Ok(addr) = target.parse::<SocketAddr>() else {
        return target.to_string();
    };
    if !addr.ip().is_unspecified() {
        return addr.to_string();
    }
    let loopback = match addr.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    SocketAddr::new(loopback, addr.port()).to_string()
}
