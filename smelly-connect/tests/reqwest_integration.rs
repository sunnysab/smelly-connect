#![cfg(feature = "test-utils")]

use std::time::Duration;

#[tokio::test]
async fn reqwest_helper_builds_client_over_session_connector() {
    let harness = smelly_connect::test_support::integration::reqwest_harness().await;
    let client = harness.session.reqwest_client().await.unwrap();
    let body = harness
        .get_with(client, "http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn reqwest_helper_reuses_internal_proxy_listener_while_clients_are_alive() {
    let harness = smelly_connect::test_support::integration::reqwest_harness().await;
    let (client_a, proxy_addr_a) =
        smelly_connect::integration::reqwest::build_client_for_test(&harness.session)
            .await
            .unwrap();
    let (client_b, proxy_addr_b) =
        smelly_connect::integration::reqwest::build_client_for_test(&harness.session)
            .await
            .unwrap();

    assert_eq!(
        proxy_addr_a, proxy_addr_b,
        "reqwest clients built from the same session should reuse one internal proxy listener"
    );

    let body = harness
        .get_with(client_a.clone(), "http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");

    drop(client_a);
    assert!(tokio::net::TcpStream::connect(proxy_addr_a).await.is_ok());

    let body = harness
        .get_with(client_b.clone(), "http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");

    drop(client_b);
    assert_proxy_eventually_stops(proxy_addr_a).await;
}

async fn assert_proxy_eventually_stops(proxy_addr: std::net::SocketAddr) {
    for _ in 0..10 {
        if tokio::net::TcpStream::connect(proxy_addr).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let result = tokio::net::TcpStream::connect(proxy_addr).await;
    assert!(
        result.is_err(),
        "internal reqwest proxy listener should stop after the last client is dropped"
    );
}
