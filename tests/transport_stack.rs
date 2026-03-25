#[tokio::test]
async fn packet_device_forwards_frames_between_channels_and_stack() {
    let harness = smelly_connect::transport::tests::packet_harness();
    harness.inject_from_vpn(vec![0, 1, 2, 3]).await;
    assert_eq!(harness.read_for_stack().await, vec![0, 1, 2, 3]);
}

#[tokio::test]
async fn stack_can_create_outbound_tcp_stream_handle() {
    let harness = smelly_connect::transport::tests::stack_harness();
    let _stream = harness.connect(("10.0.0.8", 443)).await.unwrap();
}
