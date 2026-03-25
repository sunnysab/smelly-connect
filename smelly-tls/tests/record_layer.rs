use smelly_tls::{
    build_premaster_secret, decrypt_rc4_sha1_record, derive_tls10_key_block,
    derive_tls10_master_secret, encrypt_rc4_sha1_record,
};

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
