use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};

pub const TLS11: u16 = 0x0302;
pub const TLS_RSA_WITH_RC4_128_SHA: u16 = 0x0005;
pub const TLS_EMPTY_RENEGOTIATION_INFO_SCSV: u16 = 0x00ff;
pub const HEARTBEAT_EXTENSION: u16 = 0x000f;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHelloConfig {
    pub random: [u8; 32],
    pub session_id: [u8; 32],
}

impl ClientHelloConfig {
    pub fn new(random: [u8; 32], session_id: [u8; 32]) -> Self {
        Self { random, session_id }
    }
}

pub fn build_client_hello_record(config: &ClientHelloConfig) -> Vec<u8> {
    let mut body = Vec::with_capacity(128);
    body.extend_from_slice(&TLS11.to_be_bytes());
    body.extend_from_slice(&config.random);
    body.push(config.session_id.len() as u8);
    body.extend_from_slice(&config.session_id);

    let cipher_suites = [TLS_RSA_WITH_RC4_128_SHA, TLS_EMPTY_RENEGOTIATION_INFO_SCSV];
    body.extend_from_slice(&((cipher_suites.len() * 2) as u16).to_be_bytes());
    for suite in cipher_suites {
        body.extend_from_slice(&suite.to_be_bytes());
    }

    body.push(1);
    body.push(0);

    let mut extensions = Vec::with_capacity(8);
    extensions.extend_from_slice(&HEARTBEAT_EXTENSION.to_be_bytes());
    extensions.extend_from_slice(&1_u16.to_be_bytes());
    extensions.push(1);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);

    let mut handshake = Vec::with_capacity(body.len() + 4);
    handshake.push(1);
    let body_len = body.len() as u32;
    handshake.extend_from_slice(&body_len.to_be_bytes()[1..4]);
    handshake.extend_from_slice(&body);

    let mut record = Vec::with_capacity(handshake.len() + 5);
    record.push(22);
    record.extend_from_slice(&TLS11.to_be_bytes());
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

pub fn connect_probe(addr: SocketAddr, config: &ClientHelloConfig) -> io::Result<()> {
    let mut stream = TcpStream::connect(addr)?;
    let record = build_client_hello_record(config);
    stream.write_all(&record)
}
