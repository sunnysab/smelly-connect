use std::net::Ipv4Addr;

use smelly_connect::kernel::tunnel::{
    build_recv_handshake, build_request_ip_message, build_send_handshake, derive_token,
};

#[test]
fn request_ip_message_layout_matches_existing_behavior() {
    let token = derive_token("0123456789abcdef0123456789abcdef", "abcdefghijklmnop").unwrap();
    let message = build_request_ip_message(&token);
    assert_eq!(&message[..4], &[0x00, 0x00, 0x00, 0x00]);
    assert_eq!(message.len(), 64);
}

#[test]
fn stream_handshakes_encode_client_ip_octets() {
    let token = derive_token("fedcba9876543210fedcba9876543210", "abcdefghijklmnop").unwrap();
    let send = build_send_handshake(&token, Ipv4Addr::new(10, 0, 0, 8));
    let recv = build_recv_handshake(&token, Ipv4Addr::new(10, 0, 0, 8));
    assert_eq!(send[0], 0x05);
    assert_eq!(recv[0], 0x06);
    assert_eq!(&send[60..64], &[8, 0, 0, 10]);
    assert_eq!(&recv[60..64], &[8, 0, 0, 10]);
}
