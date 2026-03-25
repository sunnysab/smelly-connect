use std::io::Read;
use std::net::TcpListener;
use std::thread;

use smelly_tls::{ClientHelloConfig, build_client_hello_record, connect_probe};

const EXPECTED_SESSION_ID: [u8; 32] = [
    b'L', b'3', b'I', b'P', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0,
];

#[test]
fn client_hello_record_matches_easyconnect_shape() {
    let config = ClientHelloConfig::new([0x11; 32], EXPECTED_SESSION_ID);
    let record = build_client_hello_record(&config);
    let parsed = parse_client_hello(&record).unwrap();

    assert_eq!(parsed.legacy_version, 0x0302);
    assert_eq!(parsed.session_id, EXPECTED_SESSION_ID);
    assert_eq!(parsed.cipher_suites, vec![0x0005, 0x00ff]);
    assert_eq!(parsed.compression_methods, vec![0]);
    assert_eq!(parsed.extension_ids, vec![0x000f]);
}

#[test]
fn connect_probe_writes_hello_bytes_to_tcp_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut header = [0_u8; 5];
        stream.read_exact(&mut header).unwrap();
        let len = u16::from_be_bytes([header[3], header[4]]) as usize;
        let mut body = vec![0_u8; len];
        stream.read_exact(&mut body).unwrap();
        [header.to_vec(), body].concat()
    });

    let config = ClientHelloConfig::new([0x22; 32], EXPECTED_SESSION_ID);
    connect_probe(addr, &config).unwrap();

    let raw = server.join().unwrap();
    let parsed = parse_client_hello(&raw).unwrap();
    assert_eq!(parsed.session_id, EXPECTED_SESSION_ID);
    assert_eq!(parsed.cipher_suites, vec![0x0005, 0x00ff]);
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "current_thread")]
async fn async_connect_probe_writes_hello_bytes_to_tcp_stream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = [0_u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut header)
            .await
            .unwrap();
        let len = u16::from_be_bytes([header[3], header[4]]) as usize;
        let mut body = vec![0_u8; len];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut body)
            .await
            .unwrap();
        [header.to_vec(), body].concat()
    });

    let config = ClientHelloConfig::new([0x33; 32], EXPECTED_SESSION_ID);
    smelly_tls::connect_hello_probe(addr, &config).await.unwrap();

    let raw = server.await.unwrap();
    let parsed = parse_client_hello(&raw).unwrap();
    assert_eq!(parsed.session_id, EXPECTED_SESSION_ID);
    assert_eq!(parsed.cipher_suites, vec![0x0005, 0x00ff]);
}

struct ParsedClientHello {
    legacy_version: u16,
    session_id: [u8; 32],
    cipher_suites: Vec<u16>,
    compression_methods: Vec<u8>,
    extension_ids: Vec<u16>,
}

fn parse_client_hello(record: &[u8]) -> Option<ParsedClientHello> {
    if record.len() < 9 || record[0] != 22 || record[5] != 1 {
        return None;
    }

    let mut idx = 9;
    let legacy_version = u16::from_be_bytes([record[idx], record[idx + 1]]);
    idx += 2;
    idx += 32;

    let session_id_len = record[idx] as usize;
    idx += 1;
    let mut session_id = [0_u8; 32];
    session_id.copy_from_slice(record.get(idx..idx + session_id_len)?);
    idx += session_id_len;

    let cipher_len = u16::from_be_bytes([record[idx], record[idx + 1]]) as usize;
    idx += 2;
    let cipher_suites = record
        .get(idx..idx + cipher_len)?
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    idx += cipher_len;

    let comp_len = record[idx] as usize;
    idx += 1;
    let compression_methods = record.get(idx..idx + comp_len)?.to_vec();
    idx += comp_len;

    let ext_len = u16::from_be_bytes([record[idx], record[idx + 1]]) as usize;
    idx += 2;
    let end = idx + ext_len;
    let mut extension_ids = Vec::new();
    while idx + 4 <= end {
        let ext_type = u16::from_be_bytes([record[idx], record[idx + 1]]);
        let ext_size = u16::from_be_bytes([record[idx + 2], record[idx + 3]]) as usize;
        extension_ids.push(ext_type);
        idx += 4 + ext_size;
    }

    Some(ParsedClientHello {
        legacy_version,
        session_id,
        cipher_suites,
        compression_methods,
        extension_ids,
    })
}
