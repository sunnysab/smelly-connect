#[tokio::test]
async fn routing_rejects_non_resource_targets_by_default() {
    let session = smelly_connect::session::tests::fake_session_without_match();
    let err = session.plan_tcp_connect(("example.com", 443)).await.unwrap_err();
    assert!(matches!(err, smelly_connect::Error::Route(_)));
}
