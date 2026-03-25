use smelly_tls::{
    build_change_cipher_spec_record, build_finished_handshake, build_premaster_secret,
    decrypt_rc4_sha1_record, derive_finished_verify_data, derive_tls10_key_block,
    derive_tls10_master_secret, encrypt_rc4_sha1_record,
};
use md5::Md5;
use sha1::Sha1;

#[test]
fn tls10_master_secret_matches_reference_prf() {
    let premaster = build_premaster_secret([0x11; 46]);
    let client_random = [0x22; 32];
    let server_random = [0x33; 32];

    let derived = derive_tls10_master_secret(&premaster, &client_random, &server_random);
    let expected = reference_tls10_prf(
        &premaster,
        b"master secret",
        &[client_random.as_slice(), server_random.as_slice()].concat(),
        48,
    );

    assert_eq!(derived.as_slice(), expected.as_slice());
}

#[test]
fn tls10_key_block_matches_reference_prf() {
    let master = [0x44; 48];
    let client_random = [0x55; 32];
    let server_random = [0x66; 32];

    let derived = derive_tls10_key_block(&master, &client_random, &server_random, 72);
    let expected = reference_tls10_prf(
        &master,
        b"key expansion",
        &[server_random.as_slice(), client_random.as_slice()].concat(),
        72,
    );

    assert_eq!(derived, expected);
}

#[test]
fn rc4_sha1_record_roundtrip_preserves_plaintext() {
    let mac_key = [0x77; 20];
    let enc_key = [0x88; 16];
    let plaintext = b"hello-easyconnect";

    let ciphertext = encrypt_rc4_sha1_record(23, 0, &mac_key, &enc_key, plaintext).unwrap();
    let decrypted = decrypt_rc4_sha1_record(23, 0, &mac_key, &enc_key, &ciphertext).unwrap();

    assert_eq!(decrypted, plaintext);
}

#[test]
fn rc4_sha1_record_rejects_tampering() {
    let mac_key = [0x12; 20];
    let enc_key = [0x34; 16];
    let plaintext = b"tamper-check";

    let mut ciphertext = encrypt_rc4_sha1_record(23, 7, &mac_key, &enc_key, plaintext).unwrap();
    ciphertext[0] ^= 0x80;

    let err = decrypt_rc4_sha1_record(23, 7, &mac_key, &enc_key, &ciphertext).unwrap_err();
    assert!(err.to_string().contains("bad record mac"));
}

#[test]
fn stateful_rc4_sha1_record_layer_handles_multiple_records() {
    let mac_key = [0x21; 20];
    let enc_key = [0x43; 16];
    let mut encryptor = smelly_tls::Rc4Sha1Encryptor::new(mac_key, enc_key);
    let mut decryptor = smelly_tls::Rc4Sha1Decryptor::new(mac_key, enc_key);

    let first = encryptor.encrypt(23, b"first-record").unwrap();
    let second = encryptor.encrypt(23, b"second-record").unwrap();

    assert_eq!(decryptor.decrypt(23, &first).unwrap(), b"first-record");
    assert_eq!(decryptor.decrypt(23, &second).unwrap(), b"second-record");
}

#[test]
fn finished_verify_data_matches_reference_prf_over_handshake_hashes() {
    let master = [0x91; 48];
    let transcript = b"clienthello...serverhello...certificate...";

    let client = derive_finished_verify_data(&master, true, transcript);
    let server = derive_finished_verify_data(&master, false, transcript);

    let handshake_hash = reference_md5_sha1(transcript);
    let expected_client =
        reference_tls10_prf(&master, b"client finished", &handshake_hash, 12);
    let expected_server =
        reference_tls10_prf(&master, b"server finished", &handshake_hash, 12);

    assert_eq!(client.as_slice(), expected_client.as_slice());
    assert_eq!(server.as_slice(), expected_server.as_slice());
}

#[test]
fn change_cipher_spec_and_finished_handshake_have_expected_shape() {
    let verify_data = [0xAB; 12];
    let ccs = build_change_cipher_spec_record();
    let finished = build_finished_handshake(verify_data);

    assert_eq!(ccs, vec![20, 0x03, 0x02, 0x00, 0x01, 0x01]);
    assert_eq!(finished[0], 20);
    assert_eq!(&finished[1..4], &[0, 0, 12]);
    assert_eq!(&finished[4..], &verify_data);
}

fn reference_tls10_prf(secret: &[u8], label: &[u8], seed: &[u8], len: usize) -> Vec<u8> {
    use hmac::Hmac;
    use md5::Md5;
    use sha1::Sha1;

    type HmacMd5 = Hmac<Md5>;
    type HmacSha1 = Hmac<Sha1>;

    let seed = [label, seed].concat();
    let left = &secret[..secret.len().div_ceil(2)];
    let right = &secret[secret.len() / 2..];

    let md5_bytes = p_hash::<HmacMd5>(left, &seed, len);
    let sha1_bytes = p_hash::<HmacSha1>(right, &seed, len);

    md5_bytes
        .iter()
        .zip(sha1_bytes.iter())
        .map(|(a, b)| a ^ b)
        .collect()
}

fn reference_md5_sha1(data: &[u8]) -> Vec<u8> {
    use md5::Digest as _;

    let md5 = Md5::digest(data);
    let sha1 = Sha1::digest(data);
    [md5.as_slice(), sha1.as_slice()].concat()
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
    let mut mac = <M as hmac::digest::KeyInit>::new_from_slice(secret).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}
