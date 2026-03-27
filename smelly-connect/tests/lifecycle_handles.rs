use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
async fn keepalive_handle_supports_explicit_shutdown() {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let session = smelly_connect::session::tests::session_with_icmp_ping(counter.clone());
    let handle = session.start_icmp_keepalive(
        smelly_connect::session::IcmpKeepAliveTarget::Ip("10.0.0.8".parse().unwrap()),
        Duration::from_millis(20),
    );

    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(counter.load(Ordering::SeqCst) >= 2);

    handle.shutdown().await.unwrap();
    let before = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(60)).await;
    let after = counter.load(Ordering::SeqCst);
    assert_eq!(after, before);
}

#[tokio::test]
async fn dropping_keepalive_handle_stops_background_task() {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let session = smelly_connect::session::tests::session_with_icmp_ping(counter.clone());
    let handle = session.start_icmp_keepalive(
        smelly_connect::session::IcmpKeepAliveTarget::Ip("10.0.0.8".parse().unwrap()),
        Duration::from_millis(20),
    );

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(counter.load(Ordering::SeqCst) >= 1);

    drop(handle);
    let before = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(60)).await;
    tokio::task::yield_now().await;
    let after = counter.load(Ordering::SeqCst);
    assert_eq!(after, before);
}

#[tokio::test]
async fn proxy_handle_supports_explicit_shutdown() {
    let session = smelly_connect::session::tests::login_harness()
        .ready_session()
        .await;
    let handle = session
        .start_http_proxy("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = handle.local_addr();

    handle.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let result = tokio::net::TcpStream::connect(addr).await;
    assert!(
        result.is_err(),
        "proxy listener should be closed after shutdown"
    );
}

#[tokio::test]
async fn dropping_last_session_clone_stops_session_owned_keepalive() {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let session = smelly_connect::session::tests::session_with_owned_keepalive(
        counter.clone(),
        Duration::from_millis(20),
    );
    let clone = session.clone();

    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(counter.load(Ordering::SeqCst) >= 2);

    drop(session);
    let before_clone_drop = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(60)).await;
    let after_clone_drop = counter.load(Ordering::SeqCst);
    assert!(
        after_clone_drop > before_clone_drop,
        "keepalive should continue while another session clone exists"
    );

    drop(clone);
    let before_final_drop = counter.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(60)).await;
    tokio::task::yield_now().await;
    let after_final_drop = counter.load(Ordering::SeqCst);
    assert_eq!(after_final_drop, before_final_drop);
}
