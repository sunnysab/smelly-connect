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
async fn dropping_reqwest_helper_client_stops_internal_proxy_listener() {
    let harness = smelly_connect::test_support::integration::reqwest_harness().await;
    let (client, proxy_addr) =
        smelly_connect::integration::reqwest::build_client_for_test(&harness.session)
            .await
            .unwrap();

    let body = harness
        .get_with(client.clone(), "http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");
    assert!(tokio::net::TcpStream::connect(proxy_addr).await.is_ok());

    drop(client);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let result = tokio::net::TcpStream::connect(proxy_addr).await;
    assert!(
        result.is_err(),
        "internal reqwest proxy listener should stop when the client is dropped"
    );
}
