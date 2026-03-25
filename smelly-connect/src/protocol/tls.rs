use std::net::Ipv4Addr;

use crate::error::ProtocolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedToken(pub [u8; 48]);

impl DerivedToken {
    pub fn as_bytes(&self) -> &[u8; 48] {
        &self.0
    }
}

pub fn derive_token(server_session_id_hex: &str, twfid: &str) -> Result<DerivedToken, ProtocolError> {
    if server_session_id_hex.len() < 31 {
        return Err(ProtocolError::InvalidSessionIdLength);
    }

    let token = format!("{}\0{twfid}", &server_session_id_hex[..31]);
    let token_bytes = token.as_bytes();
    if token_bytes.len() != 48 {
        return Err(ProtocolError::InvalidSessionIdLength);
    }

    let mut derived = [0_u8; 48];
    derived.copy_from_slice(token_bytes);
    Ok(DerivedToken(derived))
}

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
