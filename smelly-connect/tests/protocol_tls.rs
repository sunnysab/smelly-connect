#[test]
fn derive_token_matches_easyconnect_layout() {
    let token = smelly_connect::protocol::derive_token(
        "0123456789abcdef0123456789abcdef",
        "abcdefghijklmnop",
    )
    .unwrap();

    assert_eq!(
        std::str::from_utf8(token.as_bytes()).unwrap(),
        "0123456789abcdef0123456789abcde\0abcdefghijklmnop"
    );
}

#[test]
fn request_ip_message_has_expected_shape() {
    let token = smelly_connect::protocol::derive_token(
        "0123456789abcdef0123456789abcdef",
        "abcdefghijklmnop",
    )
    .unwrap();
    let message = smelly_connect::protocol::build_request_ip_message(&token);

    assert_eq!(&message[..4], &[0x00, 0x00, 0x00, 0x00]);
    assert_eq!(message.len(), 64);
    assert_eq!(
        &message[52..64],
        &[0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]
    );
}

#[test]
fn send_and_recv_handshakes_encode_reversed_ip() {
    let token = smelly_connect::protocol::derive_token(
        "fedcba9876543210fedcba9876543210",
        "abcdefghijklmnop",
    )
    .unwrap();
    let send = smelly_connect::protocol::build_send_handshake(&token, "10.0.0.8".parse().unwrap());
    let recv = smelly_connect::protocol::build_recv_handshake(&token, "10.0.0.8".parse().unwrap());

    assert_eq!(send[0], 0x05);
    assert_eq!(recv[0], 0x06);
    assert_eq!(&send[60..64], &[8, 0, 0, 10]);
    assert_eq!(&recv[60..64], &[8, 0, 0, 10]);
}
