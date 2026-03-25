use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use foreign_types::ForeignTypeRef;
use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use openssl::ssl::{ClientHelloResponse, SslAcceptor, SslMethod, SslVersion};
use openssl::x509::{X509, X509NameBuilder};
use openssl_sys as ffi;
use smelly_connect::protocol::legacy_tls::{
    EASYCONNECT_SESSION_ID, HEARTBEAT_EXT_TYPE, PROBE_EXT_TYPE, build_easyconnect_connector,
    configure_easyconnect_ssl, configure_easyconnect_ssl_probe,
};

#[derive(Debug)]
struct ObservedHello {
    legacy_version: SslVersion,
    session_id: Vec<u8>,
    compression_methods: Vec<u8>,
    heartbeat_present: bool,
    probe_ext_present: bool,
}

#[test]
fn easyconnect_clienthello_probe_sets_tls11_and_session_id() {
    let (tx, rx) = mpsc::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (cert, key) = self_signed_cert();
        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.set_certificate(&cert).unwrap();
        acceptor.set_client_hello_callback(move |ssl, _alert| {
            let observed = ObservedHello {
                legacy_version: ssl.client_hello_legacy_version().unwrap(),
                session_id: ssl.client_hello_session_id().unwrap_or_default().to_vec(),
                compression_methods: ssl
                    .client_hello_compression_methods()
                    .unwrap_or_default()
                    .to_vec(),
                heartbeat_present: has_extension(ssl, HEARTBEAT_EXT_TYPE),
                probe_ext_present: has_extension(ssl, PROBE_EXT_TYPE),
            };
            tx.send(observed).unwrap();
            Ok(ClientHelloResponse::SUCCESS)
        });
        let acceptor = acceptor.build();
        let (stream, _) = listener.accept().unwrap();
        let _ = acceptor.accept(stream);
    });

    let connector = build_easyconnect_connector().unwrap();
    let config = connector.configure().unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    configure_easyconnect_ssl_probe(&mut ssl).unwrap();
    let stream = TcpStream::connect(addr).unwrap();
    let _ = ssl.connect(stream);

    let observed = rx.recv().unwrap();
    server.join().unwrap();

    assert_eq!(observed.session_id, EASYCONNECT_SESSION_ID);
    assert_eq!(observed.legacy_version, SslVersion::TLS1_1);
    assert!(!observed.compression_methods.is_empty());
    // OpenSSL 3.6.1 on this host keeps the fixed session id and TLS 1.1 settings,
    // but does not emit our client custom extensions in the observed ClientHello.
    // This test records current behavior instead of pretending those extensions work.
    assert!(!observed.probe_ext_present);
    assert!(!observed.heartbeat_present);
}

#[test]
fn easyconnect_clienthello_probe_is_visible_on_wire() {
    let (tx, rx) = mpsc::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let hello = read_client_hello_record(&mut stream);
        tx.send(hello).unwrap();
    });

    let connector = build_easyconnect_connector().unwrap();
    let config = connector.configure().unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    configure_easyconnect_ssl_probe(&mut ssl).unwrap();
    let stream = TcpStream::connect(addr).unwrap();
    let _ = ssl.connect(stream);

    let raw = rx.recv().unwrap();
    server.join().unwrap();
    let parsed = parse_client_hello(&raw).unwrap();

    assert_eq!(parsed.legacy_version, 0x0302);
    assert_eq!(parsed.session_id, EASYCONNECT_SESSION_ID);
    assert!(!parsed.compression_methods.is_empty());
    assert!(parsed.extension_ids.contains(&PROBE_EXT_TYPE));
    assert!(parsed.extension_ids.contains(&HEARTBEAT_EXT_TYPE));
}

#[test]
fn strict_easyconnect_ssl_configuration_fails_without_rc4_support() {
    let connector = build_easyconnect_connector().unwrap();
    let config = connector.configure().unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    assert!(configure_easyconnect_ssl(&mut ssl).is_err());
}

#[test]
fn easyconnect_clienthello_probe_does_not_offer_rc4_on_this_host() {
    let (tx, rx) = mpsc::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let hello = read_client_hello_record(&mut stream);
        tx.send(hello).unwrap();
    });

    let connector = build_easyconnect_connector().unwrap();
    let config = connector.configure().unwrap();
    let mut ssl = config.into_ssl("localhost").unwrap();
    configure_easyconnect_ssl_probe(&mut ssl).unwrap();
    let stream = TcpStream::connect(addr).unwrap();
    let _ = ssl.connect(stream);

    let raw = rx.recv().unwrap();
    server.join().unwrap();
    let parsed = parse_client_hello(&raw).unwrap();
    assert!(!parsed.cipher_suites.contains(&0x0005));
}

fn has_extension(ssl: &openssl::ssl::SslRef, ext_type: u16) -> bool {
    unsafe {
        let mut out = std::ptr::null();
        let mut outlen = 0;
        ffi::SSL_client_hello_get0_ext(ssl.as_ptr(), ext_type as u32, &mut out, &mut outlen) == 1
    }
}

fn self_signed_cert() -> (X509, PKey<openssl::pkey::Private>) {
    let rsa = Rsa::generate(2048).unwrap();
    let key = PKey::from_rsa(rsa).unwrap();

    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, "localhost").unwrap();
    let name = name.build();

    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    let mut serial = BigNum::new().unwrap();
    serial.pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false).unwrap();
    let serial = serial.to_asn1_integer().unwrap();
    builder.set_serial_number(&serial).unwrap();
    builder.set_subject_name(&name).unwrap();
    builder.set_issuer_name(&name).unwrap();
    builder.set_pubkey(&key).unwrap();
    builder.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref()).unwrap();
    builder.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref()).unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();
    (builder.build(), key)
}

fn read_client_hello_record(stream: &mut TcpStream) -> Vec<u8> {
    use std::io::Read;

    let mut header = [0_u8; 5];
    stream.read_exact(&mut header).unwrap();
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body).unwrap();
    [header.to_vec(), body].concat()
}

#[derive(Debug)]
struct ParsedClientHello {
    legacy_version: u16,
    session_id: Vec<u8>,
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
    let session_id = record.get(idx..idx + session_id_len)?.to_vec();
    idx += session_id_len;

    let cipher_len = u16::from_be_bytes([record[idx], record[idx + 1]]) as usize;
    idx += 2;
    let cipher_suites = record
        .get(idx..idx + cipher_len)?
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    idx += cipher_len;

    let compression_len = record[idx] as usize;
    idx += 1;
    let compression_methods = record.get(idx..idx + compression_len)?.to_vec();
    idx += compression_len;

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
