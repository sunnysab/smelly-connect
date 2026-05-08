#[path = "../../test-support/legacy_tls.rs"]
#[allow(dead_code)]
mod legacy_tls;

#[test]
fn premaster_secret_starts_with_tls11_version() {
    let premaster = smelly_tls::build_premaster_secret([0x77; 46]);
    assert_eq!(premaster[0], 0x03);
    assert_eq!(premaster[1], 0x02);
    assert_eq!(&premaster[2..], &[0x77; 46]);
}

#[test]
fn client_key_exchange_encrypts_premaster_for_server_cert() {
    let public_key_der = legacy_tls::server_public_key_der();
    let key = legacy_tls::server_private_key();
    let premaster = smelly_tls::build_premaster_secret([0x55; 46]);
    let handshake = smelly_tls::build_client_key_exchange(&public_key_der, &premaster).unwrap();

    assert_eq!(handshake[0], 16);
    let body_len = u32::from_be_bytes([0, handshake[1], handshake[2], handshake[3]]) as usize;
    assert_eq!(body_len + 4, handshake.len());

    let encrypted_len = u16::from_be_bytes([handshake[4], handshake[5]]) as usize;
    assert_eq!(encrypted_len + 6, handshake.len());

    let encrypted = &handshake[6..];
    let decrypted = key
        .decrypt(rsa::pkcs1v15::Pkcs1v15Encrypt, encrypted)
        .unwrap();
    assert_eq!(decrypted, premaster);
}
