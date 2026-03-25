#[tokio::test]
async fn reqwest_helper_builds_client_over_session_connector() {
    let harness = smelly_connect::integration::tests::reqwest_harness().await;
    let client = harness.session.reqwest_client().await.unwrap();
    let body = harness
        .get_with(client, "http://intranet.zju.edu.cn/health")
        .await;
    assert_eq!(body, "ok");
}
