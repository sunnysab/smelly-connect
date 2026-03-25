use std::net::Ipv4Addr;

use super::DerivedToken;

pub fn build_request_ip_message(token: &DerivedToken) -> Vec<u8> {
    let mut message = vec![0x00, 0x00, 0x00, 0x00];
    message.extend_from_slice(token.as_bytes());
    message.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
    ]);
    message
}

pub fn build_send_handshake(token: &DerivedToken, client_ip: Ipv4Addr) -> Vec<u8> {
    build_stream_handshake(0x05, token, client_ip)
}

pub fn build_recv_handshake(token: &DerivedToken, client_ip: Ipv4Addr) -> Vec<u8> {
    build_stream_handshake(0x06, token, client_ip)
}

fn build_stream_handshake(kind: u8, token: &DerivedToken, client_ip: Ipv4Addr) -> Vec<u8> {
    let octets = client_ip.octets();
    let mut message = vec![kind, 0x00, 0x00, 0x00];
    message.extend_from_slice(token.as_bytes());
    message.extend_from_slice(&[0x00; 8]);
    message.extend_from_slice(&[octets[3], octets[2], octets[1], octets[0]]);
    message
}
