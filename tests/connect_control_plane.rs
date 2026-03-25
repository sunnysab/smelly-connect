#[tokio::test]
async fn connect_runs_real_control_plane_flow_against_fake_server() {
    let harness = smelly_connect::auth::tests::control_plane_harness().await;
    let session = harness.config().connect().await.unwrap();

    assert_eq!(session.client_ip().to_string(), "10.0.0.8");
    let route = session
        .plan_tcp_connect(("libdb.zju.edu.cn", 443))
        .await
        .unwrap();
    assert!(matches!(route, smelly_connect::session::RoutePlan::VpnResolved(_)));
}
