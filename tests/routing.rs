#[tokio::test]
async fn routing_rejects_non_resource_targets_by_default() {
    let session = smelly_connect::session::tests::fake_session_without_match();
    let err = session
        .plan_tcp_connect(("example.com", 443))
        .await
        .unwrap_err();
    assert!(matches!(err, smelly_connect::Error::Route(_)));
}

#[tokio::test]
async fn config_connect_builds_session_with_client_ip() {
    let harness = smelly_connect::session::tests::login_harness();
    let session = harness.config().connect().await.unwrap();
    assert_eq!(session.client_ip().to_string(), "10.0.0.8");
}
