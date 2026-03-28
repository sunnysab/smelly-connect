#[cfg(any(test, debug_assertions))]
use std::future::Future;
#[cfg(any(test, debug_assertions))]
use std::io;
use std::net::SocketAddr as StdSocketAddr;
#[cfg(any(test, debug_assertions))]
use std::net::SocketAddr;
#[cfg(any(test, debug_assertions))]
use std::sync::Arc;
use std::time::Duration;

#[cfg(any(test, debug_assertions))]
use fast_socks5::client::Socks5Datagram;
use fast_socks5::server::Socks5ServerProtocol;
use fast_socks5::{ReplyError, Socks5Command};
use fast_socks5::{new_udp_header, parse_udp_request};
use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
#[cfg(any(test, debug_assertions))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
#[cfg(any(test, debug_assertions))]
use tokio::sync::Mutex;
#[cfg(any(test, debug_assertions))]
use tokio::time::Instant;

use crate::pool::SessionPool;
#[cfg(any(test, debug_assertions))]
use crate::runtime::RuntimeSnapshot;
use crate::runtime::{ConnectionGuard, ProxyProtocol, RuntimeStats};

#[cfg(any(test, debug_assertions))]
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
enum UpstreamConnectError {
    TimedOut,
    Failed,
    RouteRejected,
}

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
#[derive(Debug, Clone)]
pub struct Socks5LiveFailureRecoveryTestResult {
    pub reply_code: u8,
    pub state_summary: String,
    pub selectable_after_failure: bool,
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
pub async fn proxy_socks5_ipv6_for_test() -> Result<Socks5ProxyTestResult, String> {
    let upstream = spawn_echo_upstream().await;
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let selected = Arc::new(Mutex::new(None::<String>));
    let addr = spawn_test_socks5(pool, {
        let selected = Arc::clone(&selected);
        move |account_name, host, _port| {
            let selected = Arc::clone(&selected);
            async move {
                if host != "::1" {
                    return Err(io::Error::other(format!("unexpected ipv6 host {host}")));
                }
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

    client
        .write_all(&[
            0x05, 0x01, 0x00, 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0x01, 0xbb,
        ])
        .await
        .map_err(|err| err.to_string())?;
    let connect_reply = read_socks5_reply(&mut client).await?;
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
pub async fn proxy_socks5_udp_associate_for_test() -> Result<Socks5ProxyTestResult, String> {
    let upstream = spawn_udp_echo_upstream().await;
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "udp.test",
        std::net::Ipv4Addr::LOCALHOST,
    );
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let addr = spawn_live_test_socks5(pool, RuntimeStats::default(), DEFAULT_CONNECT_TIMEOUT, None)
        .await?;

    let control = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    let udp_socket = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|err| err.to_string())?;
    let datagram = Socks5Datagram::use_socket(control, udp_socket)
        .await
        .map_err(|err| err.to_string())?;

    datagram
        .send_to(b"ping", ("udp.test", upstream.port()))
        .await
        .map_err(|err| err.to_string())?;
    let mut echoed = [0_u8; 64];
    let (n, _) = datagram
        .recv_from(&mut echoed)
        .await
        .map_err(|err| err.to_string())?;

    Ok(Socks5ProxyTestResult {
        account_name: "acct-01".to_string(),
        used_pool_selection: true,
        echoed_bytes: echoed[..n].to_vec(),
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_udp_associate_idle_timeout_for_test() -> Result<(), String> {
    let upstream = spawn_udp_echo_upstream().await;
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "udp.test",
        std::net::Ipv4Addr::LOCALHOST,
    );
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let addr = spawn_live_test_socks5(
        pool,
        RuntimeStats::default(),
        DEFAULT_CONNECT_TIMEOUT,
        Some(Duration::from_millis(50)),
    )
    .await?;

    let mut control = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    control
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|err| err.to_string())?;
    let mut method_reply = [0_u8; 2];
    control
        .read_exact(&mut method_reply)
        .await
        .map_err(|err| err.to_string())?;
    if method_reply != [0x05, 0x00] {
        return Err(format!("unexpected method reply: {method_reply:?}"));
    }

    control
        .write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|err| err.to_string())?;
    let reply = read_socks5_reply(&mut control).await?;
    if reply[1] != 0x00 {
        return Err(format!("unexpected socks5 reply code: {}", reply[1]));
    }
    let bind_addr = parse_reply_addr(&reply)?;

    let udp_client = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|err| err.to_string())?;
    let mut packet = new_udp_header(upstream).map_err(|err| err.to_string())?;
    packet.extend_from_slice(b"ping");
    udp_client
        .send_to(&packet, bind_addr)
        .await
        .map_err(|err| err.to_string())?;
    let mut echoed = [0_u8; 64];
    let (n, _) = tokio::time::timeout(
        Duration::from_millis(100),
        udp_client.recv_from(&mut echoed),
    )
    .await
    .map_err(|_| "udp associate did not echo before idle timeout".to_string())?
    .map_err(|err| err.to_string())?;
    let (_, _, data) = parse_udp_request(&echoed[..n])
        .await
        .map_err(|err| err.to_string())?;
    if data != b"ping" {
        return Err(format!("unexpected echoed payload: {data:?}"));
    }

    tokio::time::sleep(Duration::from_millis(80)).await;
    let mut eof = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_millis(100), control.read(&mut eof))
        .await
        .map_err(|_| "control connection did not close after idle timeout".to_string())?
        .map_err(|err| err.to_string())?;
    if read != 0 {
        return Err(format!(
            "expected closed control connection, read {read} bytes"
        ));
    }
    Ok(())
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

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_connect_failure_for_test() -> Result<Socks5FailureResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_socks5_with_timeout(
        pool,
        Duration::from_millis(20),
        |_account_name, _host, _port| async move { Err(io::Error::other("upstream failed")) },
    )
    .await?;
    request_connect_failure(addr).await
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_timeout_reply_for_test() -> Result<Socks5FailureResult, String> {
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
    request_connect_failure(addr).await
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_rejects_unsupported_methods_for_test()
-> Result<Socks5FailureResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_socks5(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
    })
    .await?;

    let mut client = TcpStream::connect(addr)
        .await
        .map_err(|err| err.to_string())?;
    client
        .write_all(&[0x05, 0x01, 0x02])
        .await
        .map_err(|err| err.to_string())?;
    let mut reply = [0_u8; 2];
    client
        .read_exact(&mut reply)
        .await
        .map_err(|err| err.to_string())?;
    Ok(Socks5FailureResult {
        reply_code: reply[1],
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_rejects_unsupported_command_for_test()
-> Result<Socks5FailureResult, String> {
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_socks5(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
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
    client
        .write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0x01, 0xbb])
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
pub async fn proxy_socks5_rejects_unsupported_atyp_for_test() -> Result<Socks5FailureResult, String>
{
    let pool = SessionPool::from_named_ready_accounts(["acct-01"]).await;
    let addr = spawn_test_socks5(pool, |_account_name, _host, _port| async move {
        Err(io::Error::other("unexpected connector use"))
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
    client
        .write_all(&[0x05, 0x01, 0x00, 0x09])
        .await
        .map_err(|err| err.to_string())?;
    let reply = read_socks5_reply(&mut client).await?;
    Ok(Socks5FailureResult {
        reply_code: reply[1],
    })
}

pub async fn serve_socks5(
    listen: String,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    udp_associate_idle_timeout: Option<Duration>,
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
            if let Err(err) = handle_live_client(
                stream,
                pool,
                stats,
                connect_timeout,
                udp_associate_idle_timeout,
            )
            .await
            {
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
        if let Err(err) =
            handle_live_client(stream, pool, stats, DEFAULT_CONNECT_TIMEOUT, None).await
        {
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
pub async fn proxy_socks5_allow_all_failure_does_not_open_for_test()
-> Result<Socks5LiveFailureRecoveryTestResult, String> {
    let session = smelly_connect::test_support::session::fake_session_without_match_with_transport(
        smelly_connect::session::EasyConnectSession::failing_transport(
            "forced allow-all target failure",
        ),
    )
    .with_allow_all_routes(true);
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let addr = spawn_live_test_socks5(
        pool.clone(),
        RuntimeStats::default(),
        DEFAULT_CONNECT_TIMEOUT,
        None,
    )
    .await?;

    let result = request_connect_failure(addr).await?;
    Ok(Socks5LiveFailureRecoveryTestResult {
        reply_code: result.reply_code,
        state_summary: pool.state_summary_for_test().await,
        selectable_after_failure: pool.has_selectable_nodes_for_test().await,
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_route_rejection_does_not_open_for_test()
-> Result<Socks5LiveFailureRecoveryTestResult, String> {
    let session = smelly_connect::test_support::session::session_with_domain_match(
        "jwxt.sit.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = SessionPool::from_live_sessions_with_keepalive_target_for_test(
        vec![("acct-01", session)],
        "10.0.0.1",
    )
    .await;
    let addr = spawn_live_test_socks5(
        pool.clone(),
        RuntimeStats::default(),
        DEFAULT_CONNECT_TIMEOUT,
        None,
    )
    .await?;

    let result = request_connect_failure_to_target(addr, "xg.sit.edu.cn", 443).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    Ok(Socks5LiveFailureRecoveryTestResult {
        reply_code: result.reply_code,
        state_summary: pool.state_summary_for_test().await,
        selectable_after_failure: pool.has_selectable_nodes_for_test().await,
    })
}

#[cfg(any(test, debug_assertions))]
pub async fn proxy_socks5_live_timeout_reply_for_test() -> Result<Socks5FailureResult, String> {
    let session = smelly_connect::test_support::session::session_with_immediate_timeout_domain_match(
        "libdb.zju.edu.cn",
        std::net::Ipv4Addr::new(10, 0, 0, 8),
    );
    let pool = SessionPool::from_live_sessions_for_test(vec![("acct-01", session)]).await;
    let addr = spawn_live_test_socks5(pool, RuntimeStats::default(), DEFAULT_CONNECT_TIMEOUT, None)
        .await?;
    request_connect_failure(addr).await
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
async fn spawn_live_test_socks5(
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    udp_associate_idle_timeout: Option<Duration>,
) -> Result<SocketAddr, String> {
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
            tokio::spawn(async move {
                let _ = handle_live_client(
                    stream,
                    pool,
                    stats,
                    connect_timeout,
                    udp_associate_idle_timeout,
                )
                .await;
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
async fn request_connect_failure(addr: SocketAddr) -> Result<Socks5FailureResult, String> {
    request_connect_failure_to_target(addr, "libdb.zju.edu.cn", 443).await
}

#[cfg(any(test, debug_assertions))]
async fn request_connect_failure_to_target(
    addr: SocketAddr,
    host: &str,
    port: u16,
) -> Result<Socks5FailureResult, String> {
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
        .write_all(&build_domain_connect_request(host, port))
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
fn build_domain_connect_request(host: &str, port: u16) -> Vec<u8> {
    let host_bytes = host.as_bytes();
    let mut request = Vec::with_capacity(7 + host_bytes.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    request.extend_from_slice(host_bytes);
    request.extend_from_slice(&port.to_be_bytes());
    request
}

#[cfg(any(test, debug_assertions))]
async fn handle_client<F, Fut>(
    client: TcpStream,
    pool: SessionPool,
    stats: Option<RuntimeStats>,
    connect_timeout: Duration,
    connector: F,
) -> Result<(), String>
where
    F: Fn(String, String, u16) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = io::Result<TcpStream>> + Send + 'static,
{
    let (proto, cmd, target_addr) = Socks5ServerProtocol::accept_no_auth(client)
        .await
        .map_err(|err| err.to_string())?
        .read_command()
        .await
        .map_err(|err| err.to_string())?;
    if cmd != Socks5Command::TCPConnect {
        proto
            .reply_error(&ReplyError::CommandNotSupported)
            .await
            .map_err(|err| err.to_string())?;
        return Ok(());
    }
    let (host, port) = target_addr.into_string_and_port();

    let account_name = match pool.next_account_name().await {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                "no ready session"
            );
            proto
                .reply_error(&ReplyError::NetworkUnreachable)
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
    let mut upstream = match connect_with_timeout(connect_timeout, upstream).await {
        Ok(upstream) => upstream,
        Err(err) => {
            proto
                .reply_error(&map_socks5_reply_error(&err))
                .await
                .map_err(|reply_err| reply_err.to_string())?;
            return Ok(());
        }
    };
    let connection = stats.map(|stats| stats.open_connection(ProxyProtocol::Socks5));
    let mut client = proto
        .reply_success("127.0.0.1:0".parse().unwrap())
        .await
        .map_err(|err| err.to_string())?;
    relay_tunnel(&mut client, &mut upstream, connection.as_ref()).await
}

async fn handle_live_client(
    client: TcpStream,
    pool: SessionPool,
    stats: RuntimeStats,
    connect_timeout: Duration,
    udp_associate_idle_timeout: Option<Duration>,
) -> Result<(), String> {
    let (proto, cmd, target_addr) = Socks5ServerProtocol::accept_no_auth(client)
        .await
        .map_err(|err| err.to_string())?
        .read_command()
        .await
        .map_err(|err| err.to_string())?;
    let (account_name, session) = match pool.next_live_session().await {
        Ok(ready) => ready,
        Err(_) => {
            tracing::warn!(
                protocol = tracing::field::display("socks5"),
                "no ready session"
            );
            proto
                .reply_error(&ReplyError::NetworkUnreachable)
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
    };

    match cmd {
        Socks5Command::TCPConnect => {
            let (host, port) = target_addr.into_string_and_port();
            tracing::info!(
                protocol = tracing::field::display("socks5"),
                target = %format!("{host}:{port}"),
                account = %account_name,
                "request accepted"
            );

            let upstream = session.connect_tcp((host.as_str(), port));
            let mut upstream = match connect_session_with_timeout(connect_timeout, upstream).await {
                Ok(upstream) => upstream,
                Err(err) => {
                    if !matches!(err, UpstreamConnectError::RouteRejected) {
                        pool.report_live_session_unhealthy_if_probe_fails(
                            &account_name,
                            &session,
                            format!("{err:?}"),
                        )
                        .await;
                    }
                    proto
                        .reply_error(&map_socks5_reply_error(&err))
                        .await
                        .map_err(|reply_err| reply_err.to_string())?;
                    return Ok(());
                }
            };
            stats.record_connect_success();
            let connection = stats.open_connection(ProxyProtocol::Socks5);
            let mut client = proto
                .reply_success("127.0.0.1:0".parse().unwrap())
                .await
                .map_err(|err| err.to_string())?;
            relay_tunnel(&mut client, &mut upstream, Some(&connection)).await
        }
        Socks5Command::UDPAssociate => {
            tracing::info!(
                protocol = tracing::field::display("socks5"),
                target = %target_addr,
                account = %account_name,
                "udp associate accepted"
            );
            let udp_socket = session.bind_udp().await.map_err(|err| format!("{err:?}"))?;
            stats.record_connect_success();
            let connection = stats.open_connection(ProxyProtocol::Socks5);
            let bind_addr = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|err| err.to_string())?;
            let reply_addr = bind_addr.local_addr().map_err(|err| err.to_string())?;
            let client = proto
                .reply_success(reply_addr)
                .await
                .map_err(|err| err.to_string())?;
            relay_udp_associate(
                client,
                bind_addr,
                udp_socket,
                udp_associate_idle_timeout,
                Some(&connection),
            )
            .await
        }
        _ => {
            proto
                .reply_error(&ReplyError::CommandNotSupported)
                .await
                .map_err(|err| err.to_string())?;
            Ok(())
        }
    }
}

async fn relay_tunnel(
    client: &mut TcpStream,
    upstream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    connection: Option<&ConnectionGuard>,
) -> Result<(), String> {
    let (client_to_upstream, upstream_to_client) = copy_bidirectional(client, upstream)
        .await
        .map_err(|err| err.to_string())?;
    if let Some(connection) = connection {
        connection.add_client_to_upstream_bytes(client_to_upstream);
        connection.add_upstream_to_client_bytes(upstream_to_client);
    }
    Ok(())
}

async fn relay_udp_associate(
    mut control: TcpStream,
    inbound: UdpSocket,
    outbound: smelly_connect::session::SessionUdpSocket,
    udp_associate_idle_timeout: Option<Duration>,
    connection: Option<&ConnectionGuard>,
) -> Result<(), String> {
    let mut client_addr = None::<StdSocketAddr>;
    let mut client_buf = vec![0_u8; 65_536];
    let mut upstream_buf = vec![0_u8; 65_536];
    let mut control_buf = [0_u8; 1];

    loop {
        tokio::select! {
            _ = async {
                match udp_associate_idle_timeout {
                    Some(timeout) => tokio::time::sleep(timeout).await,
                    None => std::future::pending::<()>().await,
                }
            } => return Ok(()),
            read = tokio::io::AsyncReadExt::read(&mut control, &mut control_buf) => {
                match read.map_err(|err| err.to_string())? {
                    0 => return Ok(()),
                    _ => return Err("unexpected control data on udp associate connection".to_string()),
                }
            }
            recv = inbound.recv_from(&mut client_buf) => {
                let (n, addr) = recv.map_err(|err| err.to_string())?;
                if let Some(expected) = client_addr {
                    if addr != expected {
                        continue;
                    }
                } else {
                    client_addr = Some(addr);
                }

                let (frag, target_addr, data) =
                    parse_udp_request(&client_buf[..n]).await.map_err(|err| err.to_string())?;
                if frag != 0 {
                    continue;
                }

                let (host, port) = target_addr.into_string_and_port();
                let sent = match outbound.send_to(data, (host, port)).await {
                    Ok(sent) => sent,
                    Err(smelly_connect::Error::RouteDecision(_)) => continue,
                    Err(smelly_connect::Error::Resolve(_)) => continue,
                    Err(err) => return Err(format!("{err:?}")),
                };
                if let Some(connection) = connection {
                    connection.add_client_to_upstream_bytes(sent as u64);
                }
            }
            recv = outbound.recv_from(&mut upstream_buf) => {
                let Some(client_addr) = client_addr else {
                    continue;
                };
                let (n, remote_addr) = recv.map_err(|err| format!("{err:?}"))?;
                let mut packet = new_udp_header(remote_addr).map_err(|err| err.to_string())?;
                packet.extend_from_slice(&upstream_buf[..n]);
                inbound
                    .send_to(&packet, client_addr)
                    .await
                    .map_err(|err| err.to_string())?;
                if let Some(connection) = connection {
                    connection.add_upstream_to_client_bytes(n as u64);
                }
            }
        }
    }
}

fn map_socks5_reply_error(_err: &UpstreamConnectError) -> ReplyError {
    match _err {
        UpstreamConnectError::TimedOut => ReplyError::ConnectionTimeout,
        UpstreamConnectError::RouteRejected => ReplyError::ConnectionNotAllowed,
        UpstreamConnectError::Failed => ReplyError::NetworkUnreachable,
    }
}

#[cfg(any(test, debug_assertions))]
async fn connect_with_timeout<T, E, Fut>(
    timeout: Duration,
    fut: Fut,
) -> Result<T, UpstreamConnectError>
where
    E: std::fmt::Debug,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_err)) => Err(UpstreamConnectError::Failed),
        Err(_) => Err(UpstreamConnectError::TimedOut),
    }
}

#[cfg(any(test, debug_assertions))]
async fn read_socks5_reply(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|err| err.to_string())?;
    let mut reply = header.to_vec();
    let remaining = match header[3] {
        0x01 => 6,
        0x04 => 18,
        0x03 => {
            let mut len = [0_u8; 1];
            stream
                .read_exact(&mut len)
                .await
                .map_err(|err| err.to_string())?;
            reply.extend_from_slice(&len);
            len[0] as usize + 2
        }
        atyp => return Err(format!("unexpected reply atyp: {atyp}")),
    };
    let mut tail = vec![0_u8; remaining];
    stream
        .read_exact(&mut tail)
        .await
        .map_err(|err| err.to_string())?;
    reply.extend_from_slice(&tail);
    Ok(reply)
}

#[cfg(any(test, debug_assertions))]
fn parse_reply_addr(reply: &[u8]) -> Result<SocketAddr, String> {
    if reply.len() < 10 {
        return Err(format!("reply too short: {}", reply.len()));
    }
    match reply[3] {
        0x01 => {
            if reply.len() != 10 {
                return Err(format!("unexpected ipv4 reply length: {}", reply.len()));
            }
            let ip = std::net::Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7]);
            let port = u16::from_be_bytes([reply[8], reply[9]]);
            Ok(SocketAddr::from((ip, port)))
        }
        atyp => Err(format!("unsupported reply atyp: {atyp}")),
    }
}

#[cfg(any(test, debug_assertions))]
async fn spawn_udp_echo_upstream() -> SocketAddr {
    let socket = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0_u8; 2048];
        loop {
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                break;
            };
            if socket.send_to(&buf[..n], peer).await.is_err() {
                break;
            }
        }
    });
    addr
}

async fn connect_session_with_timeout<Fut>(
    timeout: Duration,
    fut: Fut,
) -> Result<smelly_connect::transport::VpnStream, UpstreamConnectError>
where
    Fut: std::future::Future<
            Output = Result<smelly_connect::transport::VpnStream, smelly_connect::Error>,
        >,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(smelly_connect::Error::RouteDecision(
            smelly_connect::error::RouteDecisionError::TargetNotAllowed,
        ))) => Err(UpstreamConnectError::RouteRejected),
        Ok(Err(smelly_connect::Error::Transport(
            smelly_connect::error::TransportError::ConnectTimedOut,
        ))) => Err(UpstreamConnectError::TimedOut),
        Ok(Err(_err)) => Err(UpstreamConnectError::Failed),
        Err(_) => Err(UpstreamConnectError::TimedOut),
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
