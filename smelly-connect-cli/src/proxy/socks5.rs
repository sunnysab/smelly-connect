use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::pool::SessionPool;
use crate::runtime::{ConnectionGuard, ProxyProtocol, RuntimeSnapshot, RuntimeStats};

const SOCKS5_NETWORK_UNREACHABLE: u8 = 0x03;

#[derive(Debug, Clone)]
pub struct Socks5ProxyTestResult {
    pub account_name: String,
    pub used_pool_selection: bool,
    pub echoed_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Socks5FailureResult {
    pub reply_code: u8,
}

pub async fn proxy_socks5_for_test() -> Result<Socks5ProxyTestResult, String> {
    let upstream = spawn_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let selected = Arc::new(Mutex::new(None::<String>));
    let addr = spawn_test_socks5(pool, {
        let selected = Arc::clone(&selected);
        move |account_name, _host, _port| {
            let selected = Arc::clone(&selected);
            async move {
                *selected.lock().await = Some(account_name);
                TcpStream::connect(upstream).await
            }
        }
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|err| err.to_string())?;
    let mut method_reply = [0_u8; 2];
    client
        .read_exact(&mut method_reply)
        .await
        .map_err(|err| err.to_string())?;
    if method_reply != [0x05, 0x00] {
        return Err(format!("unexpected method reply: {method_reply:?}"));
    }

    let request = [
        0x05, 0x01, 0x00, 0x03, 0x10, b'l', b'i', b'b', b'd', b'b', b'.', b'z', b'j', b'u', b'.',
        b'e', b'd', b'u', b'.', b'c', b'n', 0x01, 0xbb,
    ];
    client
        .write_all(&request)
        .await
        .map_err(|err| err.to_string())?;
    let mut connect_reply = [0_u8; 10];
    client
        .read_exact(&mut connect_reply)
        .await
        .map_err(|err| err.to_string())?;
    if connect_reply[1] != 0x00 {
        return Err(format!(
            "unexpected socks5 reply code: {}",
            connect_reply[1]
        ));
    }

    client
        .write_all(b"ping")
        .await
        .map_err(|err| err.to_string())?;
    let mut echoed = [0_u8; 4];
    client
        .read_exact(&mut echoed)
        .await
        .map_err(|err| err.to_string())?;
    let account_name = selected
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no account selected".to_string())?;
    Ok(Socks5ProxyTestResult {
        account_name,
        used_pool_selection: true,
        echoed_bytes: echoed.to_vec(),
    })
}

pub async fn proxy_socks5_no_ready_session_for_test() -> Result<Socks5FailureResult, String> {
    let pool = SessionPool::from_failed_accounts(1).await;
    let addr = spawn_test_socks5(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    request_no_ready_session(addr).await
}

pub async fn proxy_socks5_no_ready_session_sequence_for_test(
    count: usize,
) -> Result<Vec<Socks5FailureResult>, String> {
    let pool = SessionPool::from_failed_accounts(1).await;
    let addr = spawn_test_socks5(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        results.push(request_no_ready_session(addr).await?);
    }
    Ok(results)
}

pub async fn proxy_socks5_runtime_stats_for_test() -> Result<RuntimeSnapshot, String> {
    let upstream = spawn_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let stats = RuntimeStats::default();
    let addr = spawn_test_socks5_with_stats(
        pool.clone(),
        stats.clone(),
        move |_account_name, _host, _port| async move { TcpStream::connect(upstream).await },
    )
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|err| err.to_string())?;
    let mut method_reply = [0_u8; 2];
    client
        .read_exact(&mut method_reply)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[
            0x05, 0x01, 0x00, 0x03, 0x10, b'l', b'i', b'b', b'd', b'b', b'.', b'z', b'j', b'u',
            b'.', b'e', b'd', b'u', b'.', b'c', b'n', 0x01, 0xbb,
        ])
        .await
        .map_err(|err| err.to_string())?;
    let mut connect_reply = [0_u8; 10];
    client
        .read_exact(&mut connect_reply)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(b"ping")
        .await
        .map_err(|err| err.to_string())?;
    let mut echoed = [0_u8; 4];
    client
        .read_exact(&mut echoed)
        .await
        .map_err(|err| err.to_string())?;
    client.shutdown().await.map_err(|err| err.to_string())?;
    drop(client);
    for _ in 0..10 {
        tokio::task::yield_now().await;
        let snapshot = stats.snapshot(pool.snapshot().await);
        if snapshot.socks5.current_connections == 0 {
            return Ok(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    Ok(stats.snapshot(pool.snapshot().await))
}

pub async fn serve_socks5(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
) -> Result<(), String> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|err| err.to_string())?;
    let local_addr = listener.local_addr().map_err(|err| err.to_string())?;
    tracing::info!(
        protocol = tracing::field::display("socks5"),
        listen = %local_addr,
        "socks5 proxy listening"
    );
    loop {
        let (stream, _) = listener.accept().await.map_err(|err| err.to_string())?;
        let pool = pool.clone();
        let stats = stats.clone();
        tokio::spawn(async move {
            let _ = handle_live_client(stream, pool, stats).await;
        });
    }
}

async fn spawn_test_socks5<F, Fut>(pool: SessionPool, connector: F) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_socks5_internal(pool, None, connector).await
}

async fn spawn_test_socks5_with_stats<F, Fut>(
    pool: SessionPool,
    stats: RuntimeStats,
    connector: F,
) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_socks5_internal(pool, Some(stats), connector).await
}

async fn spawn_test_socks5_internal<F, Fut>(
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connector: F,
) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let pool = pool.clone();
            let stats = stats.clone();
            let connector = connector.clone();
            tokio::spawn(async move {
                let _ = handle_client(stream, pool, stats, connector).await;
            });
        }
    });
    Ok(addr)
}

async fn request_no_ready_session(addr: SocketAddr) -> Result<Socks5FailureResult, String> {
    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|err| err.to_string())?;
    let mut method_reply = [0_u8; 2];
    client
        .read_exact(&mut method_reply)
        .await
        .map_err(|err| err.to_string())?;

    let request = [0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0x01, 0xbb];
    client
        .write_all(&request)
        .await
        .map_err(|err| err.to_string())?;
    let mut reply = [0_u8; 10];
    client
        .read_exact(&mut reply)
        .await
        .map_err(|err| err.to_string())?;
    Ok(Socks5FailureResult {
        reply_code: reply[1],
    })
}

async fn handle_client<F, Fut>(
    mut client: TcpStream,
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connector: F,
) -> Result<(), String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let mut greeting = [0_u8; 2];
    client
        .read_exact(&mut greeting)
        .await
        .map_err(|err| err.to_string())?;
    let methods_len = greeting[1] as usize;
    let mut methods = vec![0_u8; methods_len];
    client
        .read_exact(&mut methods)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|err| err.to_string())?;

    let mut header = [0_u8; 4];
    client
        .read_exact(&mut header)
        .await
        .map_err(|err| err.to_string())?;
    let atyp = header[3];
    let host = match atyp {
        0x01 => {
            let mut ip = [0_u8; 4];
            client
                .read_exact(&mut ip)
                .await
                .map_err(|err| err.to_string())?;
            Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]).to_string()
        }
        0x03 => {
            let mut len = [0_u8; 1];
            client
                .read_exact(&mut len)
                .await
                .map_err(|err| err.to_string())?;
            let mut host = vec![0_u8; len[0] as usize];
            client
                .read_exact(&mut host)
                .await
                .map_err(|err| err.to_string())?;
            String::from_utf8(host).map_err(|err| err.to_string())?
        }
        _ => return Err("unsupported atyp".to_string()),
    };
    let mut port_bytes = [0_u8; 2];
    client
        .read_exact(&mut port_bytes)
        .await
        .map_err(|err| err.to_string())?;
    let port = u16::from_be_bytes(port_bytes);

    let account_name = match pool.next_account_name().await {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                "no ready session"
            );
            client
                .write_all(&[
                    0x05,
                    SOCKS5_NETWORK_UNREACHABLE,
                    0x00,
                    0x01,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                ])
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
    };

    tracing::info!(
        protocol = tracing::field::display("socks5"),
        target = %format!("{host}:{port}"),
        account = %account_name,
        "request accepted"
    );

    let mut upstream = connector(account_name, host, port)
        .await
        .map_err(|err| err.to_string())?;
    let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Socks5));
    relay_tunnel(&mut client, &mut upstream, connection.as_ref()).await
}

async fn handle_live_client(
    mut client: TcpStream,
    pool: SessionPool,
    stats: RuntimeStats,
) -> Result<(), String> {
    let mut greeting = [0_u8; 2];
    client
        .read_exact(&mut greeting)
        .await
        .map_err(|err| err.to_string())?;
    let methods_len = greeting[1] as usize;
    let mut methods = vec![0_u8; methods_len];
    client
        .read_exact(&mut methods)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|err| err.to_string())?;

    let mut header = [0_u8; 4];
    client
        .read_exact(&mut header)
        .await
        .map_err(|err| err.to_string())?;
    let atyp = header[3];
    let host = match atyp {
        0x01 => {
            let mut ip = [0_u8; 4];
            client
                .read_exact(&mut ip)
                .await
                .map_err(|err| err.to_string())?;
            Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]).to_string()
        }
        0x03 => {
            let mut len = [0_u8; 1];
            client
                .read_exact(&mut len)
                .await
                .map_err(|err| err.to_string())?;
            let mut host = vec![0_u8; len[0] as usize];
            client
                .read_exact(&mut host)
                .await
                .map_err(|err| err.to_string())?;
            String::from_utf8(host).map_err(|err| err.to_string())?
        }
        _ => return Err("unsupported atyp".to_string()),
    };
    let mut port_bytes = [0_u8; 2];
    client
        .read_exact(&mut port_bytes)
        .await
        .map_err(|err| err.to_string())?;
    let port = u16::from_be_bytes(port_bytes);

    let (account_name, session) = match pool.next_live_session().await {
        Ok(ready) => ready,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                "no ready session"
            );
            client
                .write_all(&[
                    0x05,
                    SOCKS5_NETWORK_UNREACHABLE,
                    0x00,
                    0x01,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                ])
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
    };

    tracing::info!(
        protocol = tracing::field::display("socks5"),
        target = %format!("{host}:{port}"),
        account = %account_name,
        "request accepted"
    );

    let mut upstream = session
        .connect_tcp((host.as_str(), port))
        .await
        .map_err(|err| format!("{account_name}: {err:?}"))?;
    let connection = stats.open_connection(ProxyProtocol::Socks5);
    relay_tunnel(&mut client, &mut upstream, Some(&connection)).await
}

async fn relay_tunnel(
    client: &mut TcpStream,
    upstream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    connection: Option<&ConnectionGuard>,
) -> Result<(), String> {
    client
        .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
        .await
        .map_err(|err| err.to_string())?;
    let (client_to_upstream, upstream_to_client) = copy_bidirectional(client, upstream)
        .await
        .map_err(|err| err.to_string())?;
    if let Some(connection) = connection {
        connection.add_client_to_upstream_bytes(client_to_upstream);
        connection.add_upstream_to_client_bytes(upstream_to_client);
    }
    Ok(())
}

async fn spawn_echo_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            socket.write_all(&buf[..n]).await.unwrap();
        }
    });
    addr
}
