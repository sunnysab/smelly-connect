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

#[cfg(feature = "tokio")]
pub async fn connect_hello_probe(addr: SocketAddr, config: &ClientHelloConfig) -> io::Result<()> {
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    let record = build_client_hello_record(config);
    tokio::io::AsyncWriteExt::write_all(&mut stream, &record).await
}

#[cfg(feature = "tokio")]
pub struct ServerHelloResult {
    pub server_session_id: [u8; 32],
    pub derived_token: [u8; 48],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedServerHello {
    pub session_id: [u8; 32],
    pub cipher_suite: u16,
    pub compression_method: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFlight {
    pub server_hello: ParsedServerHello,
    pub certificate_chain: Vec<Vec<u8>>,
    pub server_hello_done: bool,
}

#[cfg(feature = "tokio")]
pub async fn connect_and_read_server_hello(
    addr: SocketAddr,
    config: &ClientHelloConfig,
    twfid: &str,
) -> io::Result<ServerHelloResult> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    let hello = build_client_hello_record(config);
    stream.write_all(&hello).await?;

    let mut header = [0_u8; 5];
    stream.read_exact(&mut header).await?;
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body).await?;
    let record = [header.to_vec(), body].concat();

    let server_session_id = parse_server_hello_session_id(&record)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid server hello"))?;
    let derived_token = derive_easyconnect_token(&server_session_id, twfid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid token shape"))?;

    Ok(ServerHelloResult {
        server_session_id,
        derived_token,
    })
}

#[cfg(feature = "tokio")]
pub async fn connect_and_read_server_flight(
    addr: SocketAddr,
    config: &ClientHelloConfig,
) -> io::Result<ServerFlight> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    let hello = build_client_hello_record(config);
    stream.write_all(&hello).await?;

    let mut header = [0_u8; 5];
    stream.read_exact(&mut header).await?;
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body).await?;
    let record = [header.to_vec(), body].concat();

    parse_server_flight(&record)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid server flight"))
}

pub fn parse_server_hello_session_id(record: &[u8]) -> Option<[u8; 32]> {
    if record.len() < 9 || record[0] != 22 || record[5] != 2 {
        return None;
    }

    let mut idx = 9;
    idx += 2;
    idx += 32;
    let sid_len = *record.get(idx)? as usize;
    idx += 1;
    if sid_len != 32 {
        return None;
    }
    let mut session_id = [0_u8; 32];
    session_id.copy_from_slice(record.get(idx..idx + sid_len)?);
    Some(session_id)
}

pub fn parse_server_flight(record: &[u8]) -> Option<ServerFlight> {
    if record.len() < 9 || record[0] != 22 {
        return None;
    }

    let mut idx = 5;
    let mut server_hello = None;
    let mut certificate_chain = Vec::new();
    let mut server_hello_done = false;

    while idx + 4 <= record.len() {
        let handshake_type = record[idx];
        let length =
            u32::from_be_bytes([0, record[idx + 1], record[idx + 2], record[idx + 3]]) as usize;
        idx += 4;
        let body = record.get(idx..idx + length)?;
        idx += length;

        match handshake_type {
            2 => {
                server_hello = Some(parse_server_hello_body(body)?);
            }
            11 => {
                certificate_chain = parse_certificate_body(body)?;
            }
            14 => {
                if !body.is_empty() {
                    return None;
                }
                server_hello_done = true;
            }
            _ => {}
        }
    }

    Some(ServerFlight {
        server_hello: server_hello?,
        certificate_chain,
        server_hello_done,
    })
}

fn parse_server_hello_body(body: &[u8]) -> Option<ParsedServerHello> {
    let mut idx = 0;
    idx += 2;
    idx += 32;
    let sid_len = *body.get(idx)? as usize;
    idx += 1;
    if sid_len != 32 {
        return None;
    }
    let mut session_id = [0_u8; 32];
    session_id.copy_from_slice(body.get(idx..idx + sid_len)?);
    idx += sid_len;
    let cipher_suite = u16::from_be_bytes([*body.get(idx)?, *body.get(idx + 1)?]);
    idx += 2;
    let compression_method = *body.get(idx)?;

    Some(ParsedServerHello {
        session_id,
        cipher_suite,
        compression_method,
    })
}

fn parse_certificate_body(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    if body.len() < 3 {
        return None;
    }
    let total_len = u32::from_be_bytes([0, body[0], body[1], body[2]]) as usize;
    let mut idx = 3;
    let end = idx + total_len;
    let mut certs = Vec::new();
    while idx + 3 <= end && idx + 3 <= body.len() {
        let cert_len =
            u32::from_be_bytes([0, body[idx], body[idx + 1], body[idx + 2]]) as usize;
        idx += 3;
        certs.push(body.get(idx..idx + cert_len)?.to_vec());
        idx += cert_len;
    }
    Some(certs)
}

pub fn derive_easyconnect_token(session_id: &[u8; 32], twfid: &str) -> Option<[u8; 48]> {
    let session_hex = hex::encode(session_id);
    let token = format!("{}\0{twfid}", &session_hex[..31]);
    let bytes = token.as_bytes();
    if bytes.len() != 48 {
        return None;
    }
    let mut out = [0_u8; 48];
    out.copy_from_slice(bytes);
    Some(out)
}
