use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::rsa::{Padding, Rsa};
use openssl::x509::{X509, X509NameBuilder};
use smelly_tls::{
    ClientHelloConfig, build_change_cipher_spec_record, build_client_hello_record,
    build_client_key_exchange, build_finished_handshake, build_premaster_secret,
    decrypt_rc4_sha1_record, derive_finished_verify_data, derive_tls10_key_block,
    derive_tls10_master_secret, parse_server_flight,
};

const CLIENT_RANDOM: [u8; 32] = [0x11; 32];
const CLIENT_SESSION_ID: [u8; 32] = [
    b'L', b'3', b'I', b'P', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0,
];
const SERVER_RANDOM: [u8; 32] = [0x22; 32];
const SERVER_SESSION_ID: [u8; 32] = *b"fedcba9876543210fedcba9876543210";

#[test]
fn client_and_server_finished_roundtrip() {
    let client_hello = build_client_hello_record(&ClientHelloConfig::new(
        CLIENT_RANDOM,
        CLIENT_SESSION_ID,
    ));
    let (cert_der, public_key_der, private_key) = server_materials();
    let server_flight_record = build_server_flight_record(
        SERVER_RANDOM,
        SERVER_SESSION_ID,
        &cert_der,
    );
    let server_flight = parse_server_flight(&server_flight_record).unwrap();

    let premaster = build_premaster_secret([0x33; 46]);
    let client_key_exchange = build_client_key_exchange(&public_key_der, &premaster).unwrap();
    let decrypted_premaster = decrypt_client_key_exchange(&client_key_exchange, &private_key);
    assert_eq!(decrypted_premaster, premaster);

    let master = derive_tls10_master_secret(&premaster, &CLIENT_RANDOM, &SERVER_RANDOM);
    let key_block = derive_tls10_key_block(&master, &CLIENT_RANDOM, &SERVER_RANDOM, 72);
    let client_mac = key_block[0..20].try_into().unwrap();
    let server_mac = key_block[20..40].try_into().unwrap();
    let client_key = key_block[40..56].try_into().unwrap();
    let server_key = key_block[56..72].try_into().unwrap();

    let mut client_transcript = Vec::new();
    client_transcript.extend_from_slice(smelly_tls::handshake_messages(&client_hello).as_slice());
    client_transcript.extend_from_slice(smelly_tls::handshake_messages(&server_flight_record).as_slice());
    client_transcript.extend_from_slice(&client_key_exchange);

    let client_verify = derive_finished_verify_data(&master, true, &client_transcript);
    let client_finished = build_finished_handshake(client_verify);
    let client_ccs = build_change_cipher_spec_record();
    let client_finished_record =
        handshake_record(smelly_tls::encrypt_rc4_sha1_record(22, 0, &client_mac, &client_key, &client_finished).unwrap());

    let server_seen_finished =
        decrypt_rc4_sha1_record(22, 0, &client_mac, &client_key, record_payload(&client_finished_record)).unwrap();
    assert_eq!(server_seen_finished, client_finished);

    client_transcript.extend_from_slice(&client_finished);
    let server_verify = derive_finished_verify_data(&master, false, &client_transcript);
    let server_finished = build_finished_handshake(server_verify);
    let server_ccs = build_change_cipher_spec_record();
    let server_finished_record =
        handshake_record(smelly_tls::encrypt_rc4_sha1_record(22, 0, &server_mac, &server_key, &server_finished).unwrap());

    let client_seen_finished =
        decrypt_rc4_sha1_record(22, 0, &server_mac, &server_key, record_payload(&server_finished_record)).unwrap();
    assert_eq!(client_seen_finished, server_finished);

    assert_eq!(client_ccs, vec![20, 0x03, 0x02, 0x00, 0x01, 0x01]);
    assert_eq!(server_ccs, vec![20, 0x03, 0x02, 0x00, 0x01, 0x01]);
    assert_eq!(server_flight.server_hello.session_id, SERVER_SESSION_ID);
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "current_thread")]
async fn async_minimal_handshake_completes_against_mock_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (cert_der, _public_key_der, private_key) = server_materials();

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
        let client_hello_record = [header.to_vec(), body].concat();

        let server_flight_record =
            build_server_flight_record(SERVER_RANDOM, SERVER_SESSION_ID, &cert_der);
        tokio::io::AsyncWriteExt::write_all(&mut stream, &server_flight_record)
            .await
            .unwrap();

        let mut hdr = [0_u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut hdr)
            .await
            .unwrap();
        let len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
        let mut cke_body = vec![0_u8; len];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut cke_body)
            .await
            .unwrap();
        let client_key_exchange_record = [hdr.to_vec(), cke_body].concat();

        let mut ccs = [0_u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut ccs)
            .await
            .unwrap();

        let mut fin_hdr = [0_u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut fin_hdr)
            .await
            .unwrap();
        let fin_len = u16::from_be_bytes([fin_hdr[3], fin_hdr[4]]) as usize;
        let mut fin_body = vec![0_u8; fin_len];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut fin_body)
            .await
            .unwrap();
        let client_finished_record = [fin_hdr.to_vec(), fin_body].concat();

        let client_handshake =
            smelly_tls::parse_client_hello(&client_hello_record).unwrap();
        let decrypted_premaster = decrypt_client_key_exchange(
            &smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap(),
            &private_key,
        );
        let master =
            smelly_tls::derive_tls10_master_secret(&decrypted_premaster, &client_handshake.random, &SERVER_RANDOM);
        let key_block =
            smelly_tls::derive_tls10_key_block(&master, &client_handshake.random, &SERVER_RANDOM, 72);
        let client_mac: [u8; 20] = key_block[0..20].try_into().unwrap();
        let server_mac: [u8; 20] = key_block[20..40].try_into().unwrap();
        let client_key: [u8; 16] = key_block[40..56].try_into().unwrap();
        let server_key: [u8; 16] = key_block[56..72].try_into().unwrap();

        let mut transcript = Vec::new();
        transcript.extend_from_slice(smelly_tls::handshake_messages(&client_hello_record).as_slice());
        transcript.extend_from_slice(smelly_tls::handshake_messages(&server_flight_record).as_slice());
        let client_key_exchange_handshake =
            smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap();
        transcript.extend_from_slice(&client_key_exchange_handshake);

        let mut server_inbound = smelly_tls::Rc4Sha1Decryptor::new(client_mac, client_key);
        let mut server_outbound = smelly_tls::Rc4Sha1Encryptor::new(server_mac, server_key);
        let client_finished_plain = server_inbound
            .decrypt(22, smelly_tls::record_payload(&client_finished_record))
            .unwrap();
        let expected_client_verify =
            smelly_tls::derive_finished_verify_data(&master, true, &transcript);
        assert_eq!(
            client_finished_plain,
            smelly_tls::build_finished_handshake(expected_client_verify)
        );

        transcript.extend_from_slice(&client_finished_plain);
        let server_verify = smelly_tls::derive_finished_verify_data(&master, false, &transcript);
        let server_finished = smelly_tls::build_finished_handshake(server_verify);
        let server_finished_record =
            smelly_tls::record_with_payload(22, &server_outbound.encrypt(22, &server_finished).unwrap());

        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            &smelly_tls::build_change_cipher_spec_record(),
        )
        .await
        .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, &server_finished_record)
            .await
            .unwrap();
    });

    let config = ClientHelloConfig::new(CLIENT_RANDOM, CLIENT_SESSION_ID);
    let result = smelly_tls::complete_minimal_handshake(addr, &config).await.unwrap();
    server.await.unwrap();

    assert_eq!(result.server_hello.session_id, SERVER_SESSION_ID);
    assert_eq!(result.master_secret.len(), 48);
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "current_thread")]
async fn async_established_connection_exchanges_application_data() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (cert_der, _public_key_der, private_key) = server_materials();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();

        let client_hello_record = read_record(&mut stream).await;
        let client_hello = smelly_tls::parse_client_hello(&client_hello_record).unwrap();
        let server_flight_record =
            build_server_flight_record(SERVER_RANDOM, SERVER_SESSION_ID, &cert_der);
        tokio::io::AsyncWriteExt::write_all(&mut stream, &server_flight_record)
            .await
            .unwrap();

        let client_key_exchange_record = read_record(&mut stream).await;
        let _ccs = read_record(&mut stream).await;
        let client_finished_record = read_record(&mut stream).await;

        let decrypted_premaster = decrypt_client_key_exchange(
            &smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap(),
            &private_key,
        );
        let master =
            smelly_tls::derive_tls10_master_secret(&decrypted_premaster, &client_hello.random, &SERVER_RANDOM);
        let key_block =
            smelly_tls::derive_tls10_key_block(&master, &client_hello.random, &SERVER_RANDOM, 72);
        let client_mac: [u8; 20] = key_block[0..20].try_into().unwrap();
        let server_mac: [u8; 20] = key_block[20..40].try_into().unwrap();
        let client_key: [u8; 16] = key_block[40..56].try_into().unwrap();
        let server_key: [u8; 16] = key_block[56..72].try_into().unwrap();

        let mut transcript = Vec::new();
        transcript.extend_from_slice(smelly_tls::handshake_messages(&client_hello_record).as_slice());
        transcript.extend_from_slice(smelly_tls::handshake_messages(&server_flight_record).as_slice());
        let client_key_exchange_handshake =
            smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap();
        transcript.extend_from_slice(&client_key_exchange_handshake);

        let mut server_inbound = smelly_tls::Rc4Sha1Decryptor::new(client_mac, client_key);
        let mut server_outbound = smelly_tls::Rc4Sha1Encryptor::new(server_mac, server_key);
        let client_finished_plain = server_inbound
            .decrypt(22, smelly_tls::record_payload(&client_finished_record))
            .unwrap();
        let expected_client_verify =
            smelly_tls::derive_finished_verify_data(&master, true, &transcript);
        assert_eq!(
            client_finished_plain,
            smelly_tls::build_finished_handshake(expected_client_verify)
        );

        transcript.extend_from_slice(&client_finished_plain);
        let server_verify = smelly_tls::derive_finished_verify_data(&master, false, &transcript);
        let server_finished = smelly_tls::build_finished_handshake(server_verify);
        let server_finished_record =
            smelly_tls::record_with_payload(22, &server_outbound.encrypt(22, &server_finished).unwrap());
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            &smelly_tls::build_change_cipher_spec_record(),
        )
        .await
        .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, &server_finished_record)
            .await
            .unwrap();

        let app_record = read_record(&mut stream).await;
        let app_plain = server_inbound
            .decrypt(23, smelly_tls::record_payload(&app_record))
            .unwrap();
        assert_eq!(app_plain, b"ping");

        let response =
            smelly_tls::record_with_payload(23, &server_outbound.encrypt(23, b"pong").unwrap());
        tokio::io::AsyncWriteExt::write_all(&mut stream, &response)
            .await
            .unwrap();
    });

    let config = ClientHelloConfig::new(CLIENT_RANDOM, CLIENT_SESSION_ID);
    let mut conn = smelly_tls::connect_tunnel(addr, &config).await.unwrap();
    conn.send_application_data(b"ping").await.unwrap();
    let response = conn.read_application_data().await.unwrap();
    assert_eq!(response, b"pong");
    server.await.unwrap();
}

fn server_materials() -> (Vec<u8>, Vec<u8>, Rsa<openssl::pkey::Private>) {
    let rsa = Rsa::generate(2048).unwrap();
    let key = PKey::from_rsa(rsa.clone()).unwrap();

    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, "localhost").unwrap();
    let name = name.build();

    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    let mut serial = BigNum::new().unwrap();
    serial
        .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
        .unwrap();
    let serial = serial.to_asn1_integer().unwrap();
    builder.set_serial_number(&serial).unwrap();
    builder.set_subject_name(&name).unwrap();
    builder.set_issuer_name(&name).unwrap();
    builder.set_pubkey(&key).unwrap();
    builder
        .set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
        .unwrap();
    builder
        .set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
        .unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();

    let cert = builder.build();
    (cert.to_der().unwrap(), key.public_key_to_der().unwrap(), rsa)
}

fn build_server_flight_record(
    server_random: [u8; 32],
    session_id: [u8; 32],
    cert_der: &[u8],
) -> Vec<u8> {
    let server_hello = build_server_hello_handshake(server_random, session_id);
    let certificate = build_certificate_handshake(cert_der);
    let server_hello_done = vec![14, 0, 0, 0];
    let payload = [server_hello, certificate, server_hello_done].concat();
    handshake_record(payload)
}

fn build_server_hello_handshake(server_random: [u8; 32], session_id: [u8; 32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0x0302_u16.to_be_bytes());
    body.extend_from_slice(&server_random);
    body.push(session_id.len() as u8);
    body.extend_from_slice(&session_id);
    body.extend_from_slice(&0x0005_u16.to_be_bytes());
    body.push(0);
    body.extend_from_slice(&0_u16.to_be_bytes());

    let mut handshake = Vec::new();
    handshake.push(2);
    let body_len = body.len() as u32;
    handshake.extend_from_slice(&body_len.to_be_bytes()[1..4]);
    handshake.extend_from_slice(&body);
    handshake
}

fn build_certificate_handshake(cert_der: &[u8]) -> Vec<u8> {
    let mut cert_list = Vec::new();
    let len = cert_der.len() as u32;
    cert_list.extend_from_slice(&len.to_be_bytes()[1..4]);
    cert_list.extend_from_slice(cert_der);

    let mut body = Vec::new();
    let cert_list_len = cert_list.len() as u32;
    body.extend_from_slice(&cert_list_len.to_be_bytes()[1..4]);
    body.extend_from_slice(&cert_list);

    let mut handshake = Vec::new();
    handshake.push(11);
    let body_len = body.len() as u32;
    handshake.extend_from_slice(&body_len.to_be_bytes()[1..4]);
    handshake.extend_from_slice(&body);
    handshake
}

fn decrypt_client_key_exchange(
    handshake: &[u8],
    private_key: &Rsa<openssl::pkey::Private>,
) -> [u8; 48] {
    assert_eq!(handshake[0], 16);
    let encrypted_len = u16::from_be_bytes([handshake[4], handshake[5]]) as usize;
    let encrypted = &handshake[6..6 + encrypted_len];
    let mut decrypted = vec![0_u8; private_key.size() as usize];
    let len = private_key
        .private_decrypt(encrypted, &mut decrypted, Padding::PKCS1)
        .unwrap();
    let mut out = [0_u8; 48];
    out.copy_from_slice(&decrypted[..len]);
    out
}

fn handshake_record(payload: Vec<u8>) -> Vec<u8> {
    let mut record = Vec::new();
    record.push(22);
    record.extend_from_slice(&0x0302_u16.to_be_bytes());
    record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    record.extend_from_slice(&payload);
    record
}

fn record_payload(record: &[u8]) -> &[u8] {
    &record[5..]
}

#[cfg(feature = "tokio")]
async fn read_record(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut header = [0_u8; 5];
    tokio::io::AsyncReadExt::read_exact(stream, &mut header)
        .await
        .unwrap();
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let mut body = vec![0_u8; len];
    tokio::io::AsyncReadExt::read_exact(stream, &mut body)
        .await
        .unwrap();
    [header.to_vec(), body].concat()
}
