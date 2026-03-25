#[tokio::test]
async fn connect_runs_real_control_plane_flow_against_fake_server() {
    let harness = smelly_connect::auth::tests::control_plane_harness().await;
    let session = harness.config().connect().await.unwrap();

    assert_eq!(session.client_ip().to_string(), "10.0.0.8");
    let route = session
        .plan_tcp_connect(("libdb.zju.edu.cn", 443))
        .await
        .unwrap();
    assert!(matches!(route, smelly_connect::session::RoutePlan::VpnResolved(_)));
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
        name.append_entry_by_nid(Nid::COMMONNAME, "localhost").unwrap();
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
        cert.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref()).unwrap();
        cert.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref()).unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();

        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.set_certificate(&cert.build()).unwrap();
        acceptor.set_min_proto_version(Some(SslVersion::TLS1_2)).unwrap();
        acceptor.set_max_proto_version(Some(SslVersion::TLS1_2)).unwrap();
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
