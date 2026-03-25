#[tokio::test]
async fn packet_device_forwards_frames_between_channels_and_stack() {
    let harness = smelly_connect::transport::tests::packet_harness();
    harness.inject_from_vpn(vec![0, 1, 2, 3]).await;
    assert_eq!(harness.read_for_stack().await, vec![0, 1, 2, 3]);
}

#[tokio::test]
async fn packet_device_forwards_frames_from_stack_to_vpn() {
    let harness = smelly_connect::transport::tests::packet_harness();
    harness.write_from_stack(vec![4, 5, 6, 7]).await;
    assert_eq!(harness.read_for_vpn().await, vec![4, 5, 6, 7]);
}

#[tokio::test]
async fn stack_can_create_outbound_tcp_stream_handle() {
    let harness = smelly_connect::transport::tests::stack_harness();
    let _stream = harness.connect(("10.0.0.8", 443)).await.unwrap();
}

#[tokio::test]
async fn session_connect_tcp_returns_async_stream() {
    let harness = smelly_connect::session::tests::login_harness();
    let session = harness.ready_session().await;
    let _stream = session.connect_tcp(("10.0.0.8", 443)).await.unwrap();
}
