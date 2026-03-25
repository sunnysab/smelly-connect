pub use crate::kernel::tunnel::DerivedToken;

pub fn derive_token(
    server_session_id_hex: &str,
    twfid: &str,
) -> Result<DerivedToken, crate::error::ProtocolError> {
    crate::kernel::tunnel::derive_token(server_session_id_hex, twfid)
}

pub fn build_request_ip_message(token: &DerivedToken) -> Vec<u8> {
    crate::kernel::tunnel::build_request_ip_message(token)
}

pub fn build_send_handshake(token: &DerivedToken, client_ip: std::net::Ipv4Addr) -> Vec<u8> {
    crate::kernel::tunnel::build_send_handshake(token, client_ip)
}

pub fn build_recv_handshake(token: &DerivedToken, client_ip: std::net::Ipv4Addr) -> Vec<u8> {
    crate::kernel::tunnel::build_recv_handshake(token, client_ip)
}
