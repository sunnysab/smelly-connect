#[tokio::test]
async fn connect_runs_real_control_plane_flow_against_fake_server() {
    let harness = smelly_connect::test_support::auth::control_plane_harness().await;
    let session = harness.config().connect().await.unwrap();

    assert_eq!(session.client_ip().to_string(), "10.0.0.8");
    let route = session
        .plan_tcp_connect(("libdb.zju.edu.cn", 443))
        .await
        .unwrap();
    assert!(matches!(
        route,
        smelly_connect::session::RoutePlan::VpnResolved(_)
    ));
}

#[test]
fn token_request_derives_token_from_tls_session_id() {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::ssl::{SslAcceptor, SslMethod, SslVersion};
    use openssl::x509::{X509, X509NameBuilder};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let rsa = Rsa::generate(2048).unwrap();
        let key = PKey::from_rsa(rsa).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
            .unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let mut serial = BigNum::new().unwrap();
        serial
            .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
            .unwrap();
        let serial = serial.to_asn1_integer().unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
            .unwrap();
        cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();

        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.set_certificate(&cert.build()).unwrap();
        acceptor
            .set_min_proto_version(Some(SslVersion::TLS1_2))
            .unwrap();
        acceptor
            .set_max_proto_version(Some(SslVersion::TLS1_2))
            .unwrap();
        let acceptor = acceptor.build();

        let (stream, _) = listener.accept().unwrap();
        let mut stream = acceptor.accept(stream).unwrap();
        let mut buf = [0_u8; 256];
        let _ = std::io::Read::read(&mut stream, &mut buf).unwrap();
        std::io::Write::write_all(&mut stream, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });

    let token = smelly_connect::auth::control::request_token(&addr.to_string(), "abcdefghijklmnop")
        .unwrap();
    assert_eq!(token.as_bytes().len(), 48);
    server.join().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn async_token_request_does_not_block_current_thread_runtime() {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::ssl::{SslAcceptor, SslMethod, SslVersion};
    use openssl::x509::{X509, X509NameBuilder};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let rsa = Rsa::generate(2048).unwrap();
        let key = PKey::from_rsa(rsa).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
            .unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let mut serial = BigNum::new().unwrap();
        serial
            .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
            .unwrap();
        let serial = serial.to_asn1_integer().unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
            .unwrap();
        cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();

        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.set_certificate(&cert.build()).unwrap();
        acceptor
            .set_min_proto_version(Some(SslVersion::TLS1_2))
            .unwrap();
        acceptor
            .set_max_proto_version(Some(SslVersion::TLS1_2))
            .unwrap();
        let acceptor = acceptor.build();

        let (stream, _) = listener.accept().unwrap();
        let mut stream = acceptor.accept(stream).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let mut buf = [0_u8; 256];
        let _ = std::io::Read::read(&mut stream, &mut buf).unwrap();
        std::io::Write::write_all(&mut stream, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });

    let running = Arc::new(AtomicBool::new(true));
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker_running = Arc::clone(&running);
    let ticker_ticks = Arc::clone(&ticks);
    let ticker = tokio::spawn(async move {
        while ticker_running.load(Ordering::SeqCst) {
            ticker_ticks.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let token = smelly_connect::auth::control::request_token_async(
        &addr.to_string(),
        "abcdefghijklmnop",
    )
    .await
    .unwrap();

    running.store(false, Ordering::SeqCst);
    ticker.await.unwrap();

    assert_eq!(token.as_bytes().len(), 48);
    assert!(
        ticks.load(Ordering::SeqCst) >= 5,
        "ticker should continue advancing while token request awaits blocking work"
    );
    server.join().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn request_ip_uses_smelly_tls_data_plane() {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::rsa::{Padding, Rsa};
    use openssl::x509::{X509, X509NameBuilder};
    use smelly_tls::{
        Rc4Sha1Decryptor, Rc4Sha1Encryptor, TLS_RSA_WITH_RC4_128_SHA,
        build_change_cipher_spec_record, build_finished_handshake, derive_finished_verify_data,
        derive_tls10_key_block, derive_tls10_master_secret, handshake_messages, record_payload,
        record_with_payload,
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = smelly_connect::protocol::derive_token(
        "0123456789abcdef0123456789abcdef",
        "abcdefghijklmnop",
    )
    .unwrap();
    let request_ip = smelly_connect::protocol::build_request_ip_message(&token);

    let server = tokio::spawn(async move {
        let rsa = Rsa::generate(2048).unwrap();
        let key = PKey::from_rsa(rsa.clone()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
            .unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let mut serial = BigNum::new().unwrap();
        serial
            .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
            .unwrap();
        let serial = serial.to_asn1_integer().unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
            .unwrap();
        cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        let cert_der = cert.build().to_der().unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let client_hello_record = read_record(&mut stream).await;
        let client_hello = smelly_tls::parse_client_hello(&client_hello_record).unwrap();
        let server_random = [0x22; 32];
        let server_session_id = *b"fedcba9876543210fedcba9876543210";
        let server_flight_record =
            build_server_flight_record(server_random, server_session_id, &cert_der);
        tokio::io::AsyncWriteExt::write_all(&mut stream, &server_flight_record)
            .await
            .unwrap();

        let client_key_exchange_record = read_record(&mut stream).await;
        let _ccs = read_record(&mut stream).await;
        let client_finished_record = read_record(&mut stream).await;
        let decrypted_premaster = decrypt_client_key_exchange(
            &smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap(),
            &rsa,
        );
        let master =
            derive_tls10_master_secret(&decrypted_premaster, &client_hello.random, &server_random);
        let key_block = derive_tls10_key_block(&master, &client_hello.random, &server_random, 72);
        let client_mac: [u8; 20] = key_block[0..20].try_into().unwrap();
        let server_mac: [u8; 20] = key_block[20..40].try_into().unwrap();
        let client_key: [u8; 16] = key_block[40..56].try_into().unwrap();
        let server_key: [u8; 16] = key_block[56..72].try_into().unwrap();
        let mut transcript = Vec::new();
        transcript.extend_from_slice(handshake_messages(&client_hello_record).as_slice());
        transcript.extend_from_slice(handshake_messages(&server_flight_record).as_slice());
        let client_key_exchange_handshake =
            smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap();
        transcript.extend_from_slice(&client_key_exchange_handshake);

        let mut server_in = Rc4Sha1Decryptor::new(client_mac, client_key);
        let mut server_out = Rc4Sha1Encryptor::new(server_mac, server_key);
        let client_finished_plain = server_in
            .decrypt(22, record_payload(&client_finished_record))
            .unwrap();
        let expected_client_verify = derive_finished_verify_data(&master, true, &transcript);
        assert_eq!(
            client_finished_plain,
            build_finished_handshake(expected_client_verify)
        );
        transcript.extend_from_slice(&client_finished_plain);
        let server_verify = derive_finished_verify_data(&master, false, &transcript);
        let server_finished = build_finished_handshake(server_verify);
        let server_finished_record =
            record_with_payload(22, &server_out.encrypt(22, &server_finished).unwrap());
        tokio::io::AsyncWriteExt::write_all(&mut stream, &build_change_cipher_spec_record())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, &server_finished_record)
            .await
            .unwrap();

        let app_record = read_record(&mut stream).await;
        let app_plain = server_in.decrypt(23, record_payload(&app_record)).unwrap();
        assert_eq!(app_plain, request_ip);

        let reply = record_with_payload(
            23,
            &server_out
                .encrypt(23, &[0x00, 0x00, 0x00, 0x00, 10, 0, 0, 8])
                .unwrap(),
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, &reply)
            .await
            .unwrap();
    });

    let ip = smelly_connect::auth::control::request_ip_via_tunnel(addr, &token, Some("RC4-SHA"))
        .await
        .unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
    server.await.unwrap();

    fn build_server_flight_record(
        server_random: [u8; 32],
        session_id: [u8; 32],
        cert_der: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0x0302_u16.to_be_bytes());
        body.extend_from_slice(&server_random);
        body.push(session_id.len() as u8);
        body.extend_from_slice(&session_id);
        body.extend_from_slice(&TLS_RSA_WITH_RC4_128_SHA.to_be_bytes());
        body.push(0);
        body.extend_from_slice(&0_u16.to_be_bytes());
        let mut server_hello = vec![2];
        server_hello.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
        server_hello.extend_from_slice(&body);

        let mut cert_list = Vec::new();
        cert_list.extend_from_slice(&(cert_der.len() as u32).to_be_bytes()[1..4]);
        cert_list.extend_from_slice(cert_der);
        let mut cert_body = Vec::new();
        cert_body.extend_from_slice(&(cert_list.len() as u32).to_be_bytes()[1..4]);
        cert_body.extend_from_slice(&cert_list);
        let mut certificate = vec![11];
        certificate.extend_from_slice(&(cert_body.len() as u32).to_be_bytes()[1..4]);
        certificate.extend_from_slice(&cert_body);

        let payload = [server_hello, certificate, vec![14, 0, 0, 0]].concat();
        let mut record = vec![22];
        record.extend_from_slice(&0x0302_u16.to_be_bytes());
        record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        record.extend_from_slice(&payload);
        record
    }

    fn decrypt_client_key_exchange(
        handshake: &[u8],
        private_key: &Rsa<openssl::pkey::Private>,
    ) -> [u8; 48] {
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
}

#[tokio::test(flavor = "current_thread")]
async fn request_ip_falls_back_to_rc4_when_hint_path_fails() {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::rsa::{Padding, Rsa};
    use openssl::x509::{X509, X509NameBuilder};
    use smelly_tls::{
        Rc4Sha1Decryptor, Rc4Sha1Encryptor, TLS_RSA_WITH_RC4_128_SHA,
        build_change_cipher_spec_record, build_finished_handshake, derive_finished_verify_data,
        derive_tls10_key_block, derive_tls10_master_secret, handshake_messages, record_payload,
        record_with_payload,
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = smelly_connect::protocol::derive_token(
        "0123456789abcdef0123456789abcdef",
        "abcdefghijklmnop",
    )
    .unwrap();
    let request_ip = smelly_connect::protocol::build_request_ip_message(&token);

    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let rsa = Rsa::generate(2048).unwrap();
            let key = PKey::from_rsa(rsa.clone()).unwrap();
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
                .unwrap();
            let name = name.build();
            let mut cert = X509::builder().unwrap();
            cert.set_version(2).unwrap();
            let mut serial = BigNum::new().unwrap();
            serial
                .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
                .unwrap();
            let serial = serial.to_asn1_integer().unwrap();
            cert.set_serial_number(&serial).unwrap();
            cert.set_subject_name(&name).unwrap();
            cert.set_issuer_name(&name).unwrap();
            cert.set_pubkey(&key).unwrap();
            cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
                .unwrap();
            cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
                .unwrap();
            cert.sign(&key, MessageDigest::sha256()).unwrap();
            let cert_der = cert.build().to_der().unwrap();

            let (mut stream, _) = listener.accept().await.unwrap();
            let client_hello_record = read_record(&mut stream).await;
            let client_hello = smelly_tls::parse_client_hello(&client_hello_record).unwrap();
            let offered = client_hello.cipher_suites[0];
            let server_random = [0x22; 32];
            let server_session_id = *b"fedcba9876543210fedcba9876543210";
            let server_flight_record =
                build_server_flight_record(server_random, server_session_id, &cert_der, offered);
            tokio::io::AsyncWriteExt::write_all(&mut stream, &server_flight_record)
                .await
                .unwrap();

            if attempt == 0 {
                let _ = read_record(&mut stream).await; // cke
                let _ = read_record(&mut stream).await; // ccs
                let _ = read_record(&mut stream).await; // finished
                tokio::io::AsyncWriteExt::write_all(
                    &mut stream,
                    &[0x15, 0x03, 0x02, 0x00, 0x02, 0x02, 0x15],
                )
                .await
                .unwrap();
                continue;
            }

            let client_key_exchange_record = read_record(&mut stream).await;
            let _ccs = read_record(&mut stream).await;
            let client_finished_record = read_record(&mut stream).await;
            let decrypted_premaster = decrypt_client_key_exchange(
                &smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap(),
                &rsa,
            );
            let master = derive_tls10_master_secret(
                &decrypted_premaster,
                &client_hello.random,
                &server_random,
            );
            let key_block =
                derive_tls10_key_block(&master, &client_hello.random, &server_random, 72);
            let client_mac: [u8; 20] = key_block[0..20].try_into().unwrap();
            let server_mac: [u8; 20] = key_block[20..40].try_into().unwrap();
            let client_key: [u8; 16] = key_block[40..56].try_into().unwrap();
            let server_key: [u8; 16] = key_block[56..72].try_into().unwrap();
            let mut transcript = Vec::new();
            transcript.extend_from_slice(handshake_messages(&client_hello_record).as_slice());
            transcript.extend_from_slice(handshake_messages(&server_flight_record).as_slice());
            let client_key_exchange_handshake =
                smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap();
            transcript.extend_from_slice(&client_key_exchange_handshake);
            let mut server_in = Rc4Sha1Decryptor::new(client_mac, client_key);
            let mut server_out = Rc4Sha1Encryptor::new(server_mac, server_key);
            let client_finished_plain = server_in
                .decrypt(22, record_payload(&client_finished_record))
                .unwrap();
            let expected_client_verify = derive_finished_verify_data(&master, true, &transcript);
            assert_eq!(
                client_finished_plain,
                build_finished_handshake(expected_client_verify)
            );
            transcript.extend_from_slice(&client_finished_plain);
            let server_verify = derive_finished_verify_data(&master, false, &transcript);
            let server_finished = build_finished_handshake(server_verify);
            let server_finished_record =
                record_with_payload(22, &server_out.encrypt(22, &server_finished).unwrap());
            tokio::io::AsyncWriteExt::write_all(&mut stream, &build_change_cipher_spec_record())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, &server_finished_record)
                .await
                .unwrap();
            let app_record = read_record(&mut stream).await;
            let app_plain = server_in.decrypt(23, record_payload(&app_record)).unwrap();
            assert_eq!(app_plain, request_ip);
            let reply = record_with_payload(
                23,
                &server_out
                    .encrypt(23, &[0x00, 0x00, 0x00, 0x00, 10, 0, 0, 8])
                    .unwrap(),
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, &reply)
                .await
                .unwrap();
            assert_eq!(offered, TLS_RSA_WITH_RC4_128_SHA);
        }
    });

    let ip = smelly_connect::auth::control::request_ip_via_tunnel(addr, &token, Some("AES128-SHA"))
        .await
        .unwrap();
    assert_eq!(ip.to_string(), "10.0.0.8");
    server.await.unwrap();

    fn build_server_flight_record(
        server_random: [u8; 32],
        session_id: [u8; 32],
        cert_der: &[u8],
        cipher: u16,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0x0302_u16.to_be_bytes());
        body.extend_from_slice(&server_random);
        body.push(session_id.len() as u8);
        body.extend_from_slice(&session_id);
        body.extend_from_slice(&cipher.to_be_bytes());
        body.push(0);
        body.extend_from_slice(&0_u16.to_be_bytes());
        let mut server_hello = vec![2];
        server_hello.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
        server_hello.extend_from_slice(&body);

        let mut cert_list = Vec::new();
        cert_list.extend_from_slice(&(cert_der.len() as u32).to_be_bytes()[1..4]);
        cert_list.extend_from_slice(cert_der);
        let mut cert_body = Vec::new();
        cert_body.extend_from_slice(&(cert_list.len() as u32).to_be_bytes()[1..4]);
        cert_body.extend_from_slice(&cert_list);
        let mut certificate = vec![11];
        certificate.extend_from_slice(&(cert_body.len() as u32).to_be_bytes()[1..4]);
        certificate.extend_from_slice(&cert_body);

        let payload = [server_hello, certificate, vec![14, 0, 0, 0]].concat();
        let mut record = vec![22];
        record.extend_from_slice(&0x0302_u16.to_be_bytes());
        record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        record.extend_from_slice(&payload);
        record
    }

    fn decrypt_client_key_exchange(
        handshake: &[u8],
        private_key: &Rsa<openssl::pkey::Private>,
    ) -> [u8; 48] {
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
}

#[tokio::test(flavor = "current_thread")]
async fn open_send_and_recv_tunnels_complete_handshakes() {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::rsa::{Padding, Rsa};
    use openssl::x509::{X509, X509NameBuilder};
    use smelly_tls::{
        Rc4Sha1Decryptor, Rc4Sha1Encryptor, TLS_RSA_WITH_RC4_128_SHA,
        build_change_cipher_spec_record, build_finished_handshake, derive_finished_verify_data,
        derive_tls10_key_block, derive_tls10_master_secret, handshake_messages, record_payload,
        record_with_payload,
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = smelly_connect::protocol::derive_token(
        "0123456789abcdef0123456789abcdef",
        "abcdefghijklmnop",
    )
    .unwrap();
    let client_ip: std::net::Ipv4Addr = "10.0.0.8".parse().unwrap();
    let send_handshake = smelly_connect::protocol::build_send_handshake(&token, client_ip);
    let recv_handshake = smelly_connect::protocol::build_recv_handshake(&token, client_ip);

    let server = tokio::spawn(async move {
        for (expected_handshake, reply_byte) in
            [(recv_handshake, 0x01_u8), (send_handshake, 0x02_u8)]
        {
            let rsa = Rsa::generate(2048).unwrap();
            let key = PKey::from_rsa(rsa.clone()).unwrap();
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
                .unwrap();
            let name = name.build();
            let mut cert = X509::builder().unwrap();
            cert.set_version(2).unwrap();
            let mut serial = BigNum::new().unwrap();
            serial
                .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
                .unwrap();
            let serial = serial.to_asn1_integer().unwrap();
            cert.set_serial_number(&serial).unwrap();
            cert.set_subject_name(&name).unwrap();
            cert.set_issuer_name(&name).unwrap();
            cert.set_pubkey(&key).unwrap();
            cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
                .unwrap();
            cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
                .unwrap();
            cert.sign(&key, MessageDigest::sha256()).unwrap();
            let cert_der = cert.build().to_der().unwrap();

            let (mut stream, _) = listener.accept().await.unwrap();
            let client_hello_record = read_record(&mut stream).await;
            let client_hello = smelly_tls::parse_client_hello(&client_hello_record).unwrap();
            let server_random = [0x22; 32];
            let server_session_id = *b"fedcba9876543210fedcba9876543210";
            let server_flight_record = build_server_flight_record(
                server_random,
                server_session_id,
                &cert_der,
                TLS_RSA_WITH_RC4_128_SHA,
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, &server_flight_record)
                .await
                .unwrap();

            let client_key_exchange_record = read_record(&mut stream).await;
            let _ccs = read_record(&mut stream).await;
            let client_finished_record = read_record(&mut stream).await;
            let decrypted_premaster = decrypt_client_key_exchange(
                &smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap(),
                &rsa,
            );
            let master = derive_tls10_master_secret(
                &decrypted_premaster,
                &client_hello.random,
                &server_random,
            );
            let key_block =
                derive_tls10_key_block(&master, &client_hello.random, &server_random, 72);
            let client_mac: [u8; 20] = key_block[0..20].try_into().unwrap();
            let server_mac: [u8; 20] = key_block[20..40].try_into().unwrap();
            let client_key: [u8; 16] = key_block[40..56].try_into().unwrap();
            let server_key: [u8; 16] = key_block[56..72].try_into().unwrap();
            let mut transcript = Vec::new();
            transcript.extend_from_slice(handshake_messages(&client_hello_record).as_slice());
            transcript.extend_from_slice(handshake_messages(&server_flight_record).as_slice());
            let client_key_exchange_handshake =
                smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap();
            transcript.extend_from_slice(&client_key_exchange_handshake);
            let mut server_in = Rc4Sha1Decryptor::new(client_mac, client_key);
            let mut server_out = Rc4Sha1Encryptor::new(server_mac, server_key);
            let client_finished_plain = server_in
                .decrypt(22, record_payload(&client_finished_record))
                .unwrap();
            let expected_client_verify = derive_finished_verify_data(&master, true, &transcript);
            assert_eq!(
                client_finished_plain,
                build_finished_handshake(expected_client_verify)
            );
            transcript.extend_from_slice(&client_finished_plain);
            let server_verify = derive_finished_verify_data(&master, false, &transcript);
            let server_finished = build_finished_handshake(server_verify);
            let server_finished_record =
                record_with_payload(22, &server_out.encrypt(22, &server_finished).unwrap());
            tokio::io::AsyncWriteExt::write_all(&mut stream, &build_change_cipher_spec_record())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, &server_finished_record)
                .await
                .unwrap();

            let app_record = read_record(&mut stream).await;
            let app_plain = server_in.decrypt(23, record_payload(&app_record)).unwrap();
            assert_eq!(app_plain, expected_handshake);

            let reply = record_with_payload(23, &server_out.encrypt(23, &[reply_byte]).unwrap());
            tokio::io::AsyncWriteExt::write_all(&mut stream, &reply)
                .await
                .unwrap();

            let extra_record = read_record(&mut stream).await;
            let extra_plain = server_in
                .decrypt(23, record_payload(&extra_record))
                .unwrap();
            let echo = record_with_payload(23, &server_out.encrypt(23, &extra_plain).unwrap());
            tokio::io::AsyncWriteExt::write_all(&mut stream, &echo)
                .await
                .unwrap();
        }
    });

    let mut recv_tunnel =
        smelly_connect::auth::control::open_recv_tunnel(addr, &token, client_ip, Some("RC4-SHA"))
            .await
            .unwrap();

    recv_tunnel.send_application_data(b"x").await.unwrap();
    let recv_echo = recv_tunnel.read_application_data().await.unwrap();
    assert_eq!(recv_echo, b"x");

    let mut send_tunnel =
        smelly_connect::auth::control::open_send_tunnel(addr, &token, client_ip, Some("RC4-SHA"))
            .await
            .unwrap();
    send_tunnel.send_application_data(b"y").await.unwrap();
    let send_echo = send_tunnel.read_application_data().await.unwrap();
    assert_eq!(send_echo, b"y");

    server.await.unwrap();

    fn build_server_flight_record(
        server_random: [u8; 32],
        session_id: [u8; 32],
        cert_der: &[u8],
        cipher: u16,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0x0302_u16.to_be_bytes());
        body.extend_from_slice(&server_random);
        body.push(session_id.len() as u8);
        body.extend_from_slice(&session_id);
        body.extend_from_slice(&cipher.to_be_bytes());
        body.push(0);
        body.extend_from_slice(&0_u16.to_be_bytes());
        let mut server_hello = vec![2];
        server_hello.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
        server_hello.extend_from_slice(&body);

        let mut cert_list = Vec::new();
        cert_list.extend_from_slice(&(cert_der.len() as u32).to_be_bytes()[1..4]);
        cert_list.extend_from_slice(cert_der);
        let mut cert_body = Vec::new();
        cert_body.extend_from_slice(&(cert_list.len() as u32).to_be_bytes()[1..4]);
        cert_body.extend_from_slice(&cert_list);
        let mut certificate = vec![11];
        certificate.extend_from_slice(&(cert_body.len() as u32).to_be_bytes()[1..4]);
        certificate.extend_from_slice(&cert_body);

        let payload = [server_hello, certificate, vec![14, 0, 0, 0]].concat();
        let mut record = vec![22];
        record.extend_from_slice(&0x0302_u16.to_be_bytes());
        record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        record.extend_from_slice(&payload);
        record
    }

    fn decrypt_client_key_exchange(
        handshake: &[u8],
        private_key: &Rsa<openssl::pkey::Private>,
    ) -> [u8; 48] {
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
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_legacy_packet_device_bridges_packets_between_stack_and_tunnels() {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::PKey;
    use openssl::rsa::{Padding, Rsa};
    use openssl::x509::{X509, X509NameBuilder};
    use smelly_tls::{
        Rc4Sha1Decryptor, Rc4Sha1Encryptor, TLS_RSA_WITH_RC4_128_SHA,
        build_change_cipher_spec_record, build_finished_handshake, derive_finished_verify_data,
        derive_tls10_key_block, derive_tls10_master_secret, handshake_messages, record_payload,
        record_with_payload,
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = smelly_connect::protocol::derive_token(
        "0123456789abcdef0123456789abcdef",
        "abcdefghijklmnop",
    )
    .unwrap();
    let client_ip: std::net::Ipv4Addr = "10.0.0.8".parse().unwrap();
    let recv_handshake = smelly_connect::protocol::build_recv_handshake(&token, client_ip);
    let send_handshake = smelly_connect::protocol::build_send_handshake(&token, client_ip);

    let server = tokio::spawn(async move {
        for (expected_handshake, reply_byte, push_after_handshake, expect_packet) in [
            (
                recv_handshake,
                0x01_u8,
                Some(vec![0xde, 0xad, 0xbe, 0xef]),
                None,
            ),
            (send_handshake, 0x02_u8, None, Some(vec![0xca, 0xfe])),
        ] {
            let rsa = Rsa::generate(2048).unwrap();
            let key = PKey::from_rsa(rsa.clone()).unwrap();
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
                .unwrap();
            let name = name.build();
            let mut cert = X509::builder().unwrap();
            cert.set_version(2).unwrap();
            let mut serial = BigNum::new().unwrap();
            serial
                .pseudo_rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
                .unwrap();
            let serial = serial.to_asn1_integer().unwrap();
            cert.set_serial_number(&serial).unwrap();
            cert.set_subject_name(&name).unwrap();
            cert.set_issuer_name(&name).unwrap();
            cert.set_pubkey(&key).unwrap();
            cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
                .unwrap();
            cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
                .unwrap();
            cert.sign(&key, MessageDigest::sha256()).unwrap();
            let cert_der = cert.build().to_der().unwrap();

            let (mut stream, _) = listener.accept().await.unwrap();
            let client_hello_record = read_record(&mut stream).await;
            let client_hello = smelly_tls::parse_client_hello(&client_hello_record).unwrap();
            let server_random = [0x22; 32];
            let server_session_id = *b"fedcba9876543210fedcba9876543210";
            let server_flight_record = build_server_flight_record(
                server_random,
                server_session_id,
                &cert_der,
                TLS_RSA_WITH_RC4_128_SHA,
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, &server_flight_record)
                .await
                .unwrap();

            let client_key_exchange_record = read_record(&mut stream).await;
            let _ccs = read_record(&mut stream).await;
            let client_finished_record = read_record(&mut stream).await;
            let decrypted_premaster = decrypt_client_key_exchange(
                &smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap(),
                &rsa,
            );
            let master = derive_tls10_master_secret(
                &decrypted_premaster,
                &client_hello.random,
                &server_random,
            );
            let key_block =
                derive_tls10_key_block(&master, &client_hello.random, &server_random, 72);
            let client_mac: [u8; 20] = key_block[0..20].try_into().unwrap();
            let server_mac: [u8; 20] = key_block[20..40].try_into().unwrap();
            let client_key: [u8; 16] = key_block[40..56].try_into().unwrap();
            let server_key: [u8; 16] = key_block[56..72].try_into().unwrap();
            let mut transcript = Vec::new();
            transcript.extend_from_slice(handshake_messages(&client_hello_record).as_slice());
            transcript.extend_from_slice(handshake_messages(&server_flight_record).as_slice());
            let client_key_exchange_handshake =
                smelly_tls::parse_single_handshake(&client_key_exchange_record).unwrap();
            transcript.extend_from_slice(&client_key_exchange_handshake);
            let mut server_in = Rc4Sha1Decryptor::new(client_mac, client_key);
            let mut server_out = Rc4Sha1Encryptor::new(server_mac, server_key);
            let client_finished_plain = server_in
                .decrypt(22, record_payload(&client_finished_record))
                .unwrap();
            let expected_client_verify = derive_finished_verify_data(&master, true, &transcript);
            assert_eq!(
                client_finished_plain,
                build_finished_handshake(expected_client_verify)
            );
            transcript.extend_from_slice(&client_finished_plain);
            let server_verify = derive_finished_verify_data(&master, false, &transcript);
            let server_finished = build_finished_handshake(server_verify);
            let server_finished_record =
                record_with_payload(22, &server_out.encrypt(22, &server_finished).unwrap());
            tokio::io::AsyncWriteExt::write_all(&mut stream, &build_change_cipher_spec_record())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, &server_finished_record)
                .await
                .unwrap();

            let handshake_record = read_record(&mut stream).await;
            let handshake_plain = server_in
                .decrypt(23, record_payload(&handshake_record))
                .unwrap();
            assert_eq!(handshake_plain, expected_handshake);
            let reply = record_with_payload(23, &server_out.encrypt(23, &[reply_byte]).unwrap());
            tokio::io::AsyncWriteExt::write_all(&mut stream, &reply)
                .await
                .unwrap();

            if let Some(packet) = push_after_handshake {
                let push = record_with_payload(23, &server_out.encrypt(23, &packet).unwrap());
                tokio::io::AsyncWriteExt::write_all(&mut stream, &push)
                    .await
                    .unwrap();
            }

            if let Some(packet) = expect_packet {
                let app_record = read_record(&mut stream).await;
                let app_plain = server_in.decrypt(23, record_payload(&app_record)).unwrap();
                assert_eq!(app_plain, packet);
            }
        }
    });

    let device = smelly_connect::auth::control::spawn_legacy_packet_device(
        addr,
        &token,
        client_ip,
        Some("RC4-SHA"),
    )
    .await
    .unwrap();

    let inbound = device.read_for_stack().await.unwrap();
    assert_eq!(inbound, vec![0xde, 0xad, 0xbe, 0xef]);
    device.write_from_stack(vec![0xca, 0xfe]).await;
    server.await.unwrap();

    fn build_server_flight_record(
        server_random: [u8; 32],
        session_id: [u8; 32],
        cert_der: &[u8],
        cipher: u16,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0x0302_u16.to_be_bytes());
        body.extend_from_slice(&server_random);
        body.push(session_id.len() as u8);
        body.extend_from_slice(&session_id);
        body.extend_from_slice(&cipher.to_be_bytes());
        body.push(0);
        body.extend_from_slice(&0_u16.to_be_bytes());
        let mut server_hello = vec![2];
        server_hello.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
        server_hello.extend_from_slice(&body);

        let mut cert_list = Vec::new();
        cert_list.extend_from_slice(&(cert_der.len() as u32).to_be_bytes()[1..4]);
        cert_list.extend_from_slice(cert_der);
        let mut cert_body = Vec::new();
        cert_body.extend_from_slice(&(cert_list.len() as u32).to_be_bytes()[1..4]);
        cert_body.extend_from_slice(&cert_list);
        let mut certificate = vec![11];
        certificate.extend_from_slice(&(cert_body.len() as u32).to_be_bytes()[1..4]);
        certificate.extend_from_slice(&cert_body);

        let payload = [server_hello, certificate, vec![14, 0, 0, 0]].concat();
        let mut record = vec![22];
        record.extend_from_slice(&0x0302_u16.to_be_bytes());
        record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        record.extend_from_slice(&payload);
        record
    }

    fn decrypt_client_key_exchange(
        handshake: &[u8],
        private_key: &Rsa<openssl::pkey::Private>,
    ) -> [u8; 48] {
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
}
