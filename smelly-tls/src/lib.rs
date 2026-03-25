use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};

use hmac::{Hmac, Mac};
use md5::Md5;
use rc4::{KeyInit as Rc4KeyInit, Rc4, StreamCipher};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use sha1::Sha1;

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

pub fn build_premaster_secret(random_tail: [u8; 46]) -> [u8; 48] {
    let mut premaster = [0_u8; 48];
    premaster[..2].copy_from_slice(&TLS11.to_be_bytes());
    premaster[2..].copy_from_slice(&random_tail);
    premaster
}

pub fn encrypt_premaster_secret(
    public_key_der: &[u8],
    premaster: &[u8; 48],
) -> io::Result<Vec<u8>> {
    let public_key = RsaPublicKey::from_public_key_der(public_key_der)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    let mut rng = rsa::rand_core::OsRng;
    public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, premaster)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

pub fn build_client_key_exchange(
    public_key_der: &[u8],
    premaster: &[u8; 48],
) -> io::Result<Vec<u8>> {
    let encrypted = encrypt_premaster_secret(public_key_der, premaster)?;

    let mut body = Vec::with_capacity(encrypted.len() + 2);
    body.extend_from_slice(&(encrypted.len() as u16).to_be_bytes());
    body.extend_from_slice(&encrypted);

    let mut handshake = Vec::with_capacity(body.len() + 4);
    handshake.push(16);
    let body_len = body.len() as u32;
    handshake.extend_from_slice(&body_len.to_be_bytes()[1..4]);
    handshake.extend_from_slice(&body);
    Ok(handshake)
}

pub fn derive_tls10_master_secret(
    premaster: &[u8; 48],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
) -> [u8; 48] {
    let seed = [client_random.as_slice(), server_random.as_slice()].concat();
    let bytes = tls10_prf(premaster, b"master secret", &seed, 48);
    let mut out = [0_u8; 48];
    out.copy_from_slice(&bytes);
    out
}

pub fn derive_tls10_key_block(
    master_secret: &[u8; 48],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
    len: usize,
) -> Vec<u8> {
    let seed = [server_random.as_slice(), client_random.as_slice()].concat();
    tls10_prf(master_secret, b"key expansion", &seed, len)
}

pub fn encrypt_rc4_sha1_record(
    content_type: u8,
    sequence_number: u64,
    mac_key: &[u8; 20],
    enc_key: &[u8; 16],
    plaintext: &[u8],
) -> io::Result<Vec<u8>> {
    let mac = tls10_record_mac(mac_key, sequence_number, content_type, plaintext)?;
    let mut payload = Vec::with_capacity(plaintext.len() + mac.len());
    payload.extend_from_slice(plaintext);
    payload.extend_from_slice(&mac);
    apply_rc4(enc_key, &mut payload)?;
    Ok(payload)
}

pub fn decrypt_rc4_sha1_record(
    content_type: u8,
    sequence_number: u64,
    mac_key: &[u8; 20],
    enc_key: &[u8; 16],
    ciphertext: &[u8],
) -> io::Result<Vec<u8>> {
    if ciphertext.len() < 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "record too short"));
    }

    let mut payload = ciphertext.to_vec();
    apply_rc4(enc_key, &mut payload)?;
    let split = payload.len() - 20;
    let plaintext = payload[..split].to_vec();
    let received_mac = &payload[split..];
    let expected_mac = tls10_record_mac(mac_key, sequence_number, content_type, &plaintext)?;
    if received_mac != expected_mac.as_slice() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad record mac"));
    }
    Ok(plaintext)
}

fn tls10_prf(secret: &[u8], label: &[u8], seed: &[u8], len: usize) -> Vec<u8> {
    let full_seed = [label, seed].concat();
    let left = &secret[..secret.len().div_ceil(2)];
    let right = &secret[secret.len() / 2..];

    let md5_bytes = p_hash::<Hmac<Md5>>(left, &full_seed, len);
    let sha1_bytes = p_hash::<Hmac<Sha1>>(right, &full_seed, len);

    md5_bytes
        .iter()
        .zip(sha1_bytes.iter())
        .map(|(a, b)| a ^ b)
        .collect()
}

fn p_hash<M>(secret: &[u8], seed: &[u8], len: usize) -> Vec<u8>
where
    M: hmac::digest::KeyInit + hmac::Mac + Clone,
{
    let mut out = Vec::with_capacity(len);
    let mut a = hmac_once::<M>(secret, seed);
    while out.len() < len {
        let mut block_seed = Vec::with_capacity(a.len() + seed.len());
        block_seed.extend_from_slice(&a);
        block_seed.extend_from_slice(seed);
        out.extend_from_slice(&hmac_once::<M>(secret, &block_seed));
        a = hmac_once::<M>(secret, &a);
    }
    out.truncate(len);
    out
}

fn hmac_once<M>(secret: &[u8], data: &[u8]) -> Vec<u8>
where
    M: hmac::digest::KeyInit + hmac::Mac,
{
    let mut mac = <M as hmac::digest::KeyInit>::new_from_slice(secret)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid hmac key"))
        .unwrap();
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn tls10_record_mac(
    mac_key: &[u8; 20],
    sequence_number: u64,
    content_type: u8,
    plaintext: &[u8],
) -> io::Result<Vec<u8>> {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(mac_key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    mac.update(&sequence_number.to_be_bytes());
    mac.update(&[content_type]);
    mac.update(&TLS11.to_be_bytes());
    mac.update(&(plaintext.len() as u16).to_be_bytes());
    mac.update(plaintext);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn apply_rc4(key: &[u8; 16], payload: &mut [u8]) -> io::Result<()> {
    let mut cipher =
        Rc4::<rc4::consts::U16>::new_from_slice(key).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    cipher.apply_keystream(payload);
    Ok(())
}
