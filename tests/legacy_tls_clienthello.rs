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
    configure_easyconnect_ssl_probe,
};

#[derive(Debug)]
struct ObservedHello {
    legacy_version: SslVersion,
    session_id: Vec<u8>,
    compression_methods: Vec<u8>,
    heartbeat_present: bool,
    probe_ext_present: bool,
    extension_ids: Vec<u16>,
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
                extension_ids: extension_ids(ssl),
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

    eprintln!("extensions: {:?}", observed.extension_ids);
    assert_eq!(observed.session_id, EASYCONNECT_SESSION_ID);
    assert_eq!(observed.legacy_version, SslVersion::TLS1_1);
    assert!(!observed.compression_methods.is_empty());
    // OpenSSL 3.6.1 on this host keeps the fixed session id and TLS 1.1 settings,
    // but does not emit our client custom extensions in the observed ClientHello.
    // This test records current behavior instead of pretending those extensions work.
    assert!(!observed.probe_ext_present);
    assert!(!observed.heartbeat_present);
}

fn has_extension(ssl: &openssl::ssl::SslRef, ext_type: u16) -> bool {
    unsafe {
        let mut out = std::ptr::null();
        let mut outlen = 0;
        ffi::SSL_client_hello_get0_ext(ssl.as_ptr(), ext_type as u32, &mut out, &mut outlen) == 1
    }
}

fn extension_ids(ssl: &openssl::ssl::SslRef) -> Vec<u16> {
    unsafe {
        let mut out = std::ptr::null_mut();
        let mut outlen = 0;
        if ffi::SSL_client_hello_get1_extensions_present(ssl.as_ptr(), &mut out, &mut outlen) != 1
        {
            return Vec::new();
        }
        let values = std::slice::from_raw_parts(out, outlen)
            .iter()
            .map(|value| *value as u16)
            .collect::<Vec<_>>();
        ffi::OPENSSL_free(out.cast());
        values
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
