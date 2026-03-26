#[cfg(any(test, debug_assertions))]
use std::future::Future;
#[cfg(any(test, debug_assertions))]
use std::io;
use std::net::Ipv4Addr;
#[cfg(any(test, debug_assertions))]
use std::net::SocketAddr;
#[cfg(any(test, debug_assertions))]
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
#[cfg(any(test, debug_assertions))]
use tokio::sync::Mutex;
#[cfg(any(test, debug_assertions))]
use tokio::time::Instant;

use crate::pool::SessionPool;
#[cfg(any(test, debug_assertions))]
use crate::runtime::RuntimeSnapshot;
use crate::runtime::{ConnectionGuard, ProxyProtocol, RuntimeStats};

const SOCKS5_NETWORK_UNREACHABLE: u8 = 0x03;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct Socks5ProxyTestResult {
    pub account_name: String,
    pub used_pool_selection: bool,
    pub echoed_bytes: Vec<u8>,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct Socks5FailureResult {
    pub reply_code: u8,
}

#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone)]
pub struct TimeoutTestResult {
    pub elapsed: Duration,
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_no_ready_session_for_test() -> Result<Socks5FailureResult, String> {
    let pool = SessionPool::from_failed_accounts(1).await;
    let addr = spawn_test_socks5(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    request_no_ready_session(addr).await
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
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
        let snapshot = stats.snapshot(pool.summary().await);
        if snapshot.socks5.current_connections == 0 {
            return Ok(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    Ok(stats.snapshot(pool.summary().await))
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_connect_timeout_for_test() -> Result<TimeoutTestResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_socks5_with_timeout(
        pool,
        Duration::from_millis(20),
        |_account_name, _host, _port| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Err(io::Error::new(io::ErrorKind::TimedOut, "slow upstream"))
        },
    )
    .await?;

    let started = Instant::now();
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
    let mut response = Vec::new();
    client
        .read_to_end(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    Ok(TimeoutTestResult {
        elapsed: started.elapsed(),
    })
}

pub async fn serve_socks5(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
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
        let connect_timeout = connect_timeout;
        tokio::spawn(async move {
            if let Err(err) = handle_live_client(stream, pool, stats, connect_timeout).await {
                tracing::warn!(
                    protocol = tracing::field::display("socks5"),
                    error = %err,
                    "live proxy request failed"
                );
            }
        });
    }
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_live_failure_for_test() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let pool = SessionPool::from_failed_accounts(1).await;
    let stats = RuntimeStats::default();

    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        if let Err(err) = handle_live_client(stream, pool, stats, DEFAULT_CONNECT_TIMEOUT).await {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                error = %err,
                "live proxy request failed"
            );
        }
    });

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x01, 0x00, 0x05, 0x00, 0x00, 0x00])
        .await
        .map_err(|err| err.to_string())?;
    let _ = client.shutdown().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    Ok(())
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_socks5<F, Fut>(pool: SessionPool, connector: F) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_socks5_internal(pool, None, DEFAULT_CONNECT_TIMEOUT, connector).await
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_socks5_with_stats<F, Fut>(
    pool: SessionPool,
    stats: RuntimeStats,
    connector: F,
) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_socks5_internal(pool, Some(stats), DEFAULT_CONNECT_TIMEOUT, connector).await
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_socks5_with_timeout<F, Fut>(
    pool: SessionPool,
    connect_timeout: Duration,
    connector: F,
) -> Result<SocketAddr, String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    spawn_test_socks5_internal(pool, None, connect_timeout, connector).await
}

#[cfg(any(test, debug_assertions))]
async fn spawn_test_socks5_internal<F, Fut>(
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connect_timeout: Duration,
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
            let connect_timeout = connect_timeout;
            tokio::spawn(async move {
                let _ = handle_client(stream, pool, stats, connect_timeout, connector).await;
            });
        }
    });
    Ok(addr)
}

#[cfg(any(test, debug_assertions))]
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

#[cfg(any(test, debug_assertions))]
async fn handle_client<F, Fut>(
    mut client: TcpStream,
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connect_timeout: Duration,
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

    let upstream = connector(account_name, host, port);
    let mut upstream = connect_with_timeout(connect_timeout, upstream).await?;
    let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Socks5));
    relay_tunnel(&mut client, &mut upstream, connection.as_ref()).await
}

async fn handle_live_client(
    mut client: TcpStream,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
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

    let upstream = session.connect_tcp((host.as_str(), port));
    let mut upstream = connect_with_timeout(connect_timeout, upstream)
        .await
        .map_err(|err| format!("{account_name}: {err}"))?;
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

async fn connect_with_timeout<T, E, Fut>(timeout: Duration, fut: Fut) -> Result<T, String>
where
    E: std::fmt::Debug,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(format!("{err:?}")),
        Err(_) => Err(format!("connect timed out after {}ms", timeout.as_millis())),
    }
}

#[cfg(any(test, debug_assertions))]
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
