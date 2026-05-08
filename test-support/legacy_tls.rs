#![allow(dead_code)]

use std::sync::OnceLock;

use rsa::RsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Encrypt;
use rsa::pkcs8::DecodePrivateKey;

const SERVER_PRIVATE_KEY_DER_HEX: &str = concat!(
    "308204be020100300d06092a864886f70d0101010500048204a8308204a40201000282010100",
    "ce62e4d7c07c9f5fde5df7d0e7f8782fd30092b6cb8b4ecece7c0e46093db39dfb3069c58575",
    "d2b9890de744ccab5a3cf8d1c08f4b39d3a327be4f1e011f7f84672c78b74d67c417a45e30d7",
    "db6ac3893115e1b0531fa763a315415503c20549670baf7d5dca4dad09ac1310a5fea4dec496",
    "68efcbb731aaa8f954ea1ef6aa792e45c6283be60ba0206e8b61572307f342eef77eeeb4fd45",
    "094ace6f282c9e49fb0f38dd9078383a8e393e594e048512d7960efbee4efbce711f115fcfb2",
    "65eeb057871e0b41ed38b9fa06a62b248295d2a58b2a980c6bcd2005448c0024c4c4e6fb3a36",
    "73570ee34808a12e51872ba51b24aeddbae550779490d81178a0423302030100010282010054",
    "866313cb951e8e9c5b1ec9d39f4ad5c7546641f005cb4e5e79a73cdebf02e7923f072aaa98912",
    "7846e89c77f0d34856120427d4d414f20747ff816213e9db283b0ce75c0015de992db32a4cd0",
    "ba22e77486f688ffa984af1b91c4b2da15219f8566c566b4995db66e92edbb42820accd958e3",
    "f9b4e994c3c2cf52e7764ed76bc87f60987f129806c7aa43078ef8e76ff988930eb4e62f6ab9",
    "617c8d2bdd536a29e192a4bb62368486c918b4c75c762ea14da9f63f3ea4b2abf48f4636421b",
    "4cde5b4c24a85cbf7b5b47ab0dcc0eda5a8fa03e068c621f45fc032547795ee5a74f387d82ad",
    "e6281d53e5da6a2d8fdce3e87de17bf62602ad14da4757fbb50e902818100e7eed27319872625",
    "1c1f350bc03b38027b6b9b90b232833f881d2faf2b8259f6103be4306595d1c83dea048e9833",
    "d8be81f119311bd9e0ff12ae214a0e034040c7f1b035c7b16fac4b423f526f9d9cf941377310",
    "1c6d0fa29a57e583002c95a84304205da30c84697d73cdc6cb8c246df165b34cebd68016ad68",
    "11a00cdbd40902818100e3cd7092733d751391dc3324b8a2c12ceb661bcee8cebd0ff5242f05",
    "17d15bad3c6198223b2f65dd903a876aac7ef595360d7327b5d3a564d384b7a4ac3e31ceaa1b",
    "2e3ffc424071c5caaa1d3e536e87efc45b0b9f1cc2dc1c18fc59b95ca6bb48f5cf7f68a71535",
    "fc0ae8ebf4c1a5acb6abcd09ff244260a686ce5bac438b5b028181009645787936cb07fdf750",
    "88f00a26d44d57660b2f1f25f73fcc930c78347a8a8b114d9cb576bc3165ca27bbb82009479f",
    "77559cfae28eb266d1d59c9ffca0429b3670f3b884a00438dfb21690f4dc6bfe5b30f00e3a3c",
    "b76aa511da149ca2467cf49ed1d19978dcb9f49c79711a24bcddb7102bb1e503df8dd1e0a1ba",
    "cf5a06f102818100bbc27bd3a9ba71022549eab98c99514175f09e61075047529cca2b1b368b",
    "6fd5b49bf829d6c075648e593f7e34191ccfd45277a4b66070c54ef5e7eb89b0659b2267eed1",
    "fc589b076b7064905feba281d6a4f029ff0654b1d952dac4155d016c0271e089f2372ceb6707",
    "92fdd0a9bfa540971013fa40e7990408be939ec3b95b02818035b3cf11fc9985f072d8a6e882",
    "af8fde847a9906aea0d9552f6ebe777c9baf7cce5830b6decaae7107580f6f23d362dc8c3269",
    "93ac90ffc9fb0ed461c24fdaea071ac8e52ae01c1b1b2fe24da65d7c23b798b9747bb80dbfa5",
    "28493883bba026391921cd703dd79f67ee0f402bad29dc4bc4b493c4ff6e25943034280bc89010"
);

const SERVER_PUBLIC_KEY_DER_HEX: &str = concat!(
    "30820122300d06092a864886f70d01010105000382010f003082010a0282010100ce62e4d7c0",
    "7c9f5fde5df7d0e7f8782fd30092b6cb8b4ecece7c0e46093db39dfb3069c58575d2b9890de7",
    "44ccab5a3cf8d1c08f4b39d3a327be4f1e011f7f84672c78b74d67c417a45e30d7db6ac38931",
    "15e1b0531fa763a315415503c20549670baf7d5dca4dad09ac1310a5fea4dec49668efcbb731",
    "aaa8f954ea1ef6aa792e45c6283be60ba0206e8b61572307f342eef77eeeb4fd45094ace6f28",
    "2c9e49fb0f38dd9078383a8e393e594e048512d7960efbee4efbce711f115fcfb265eeb05787",
    "1e0b41ed38b9fa06a62b248295d2a58b2a980c6bcd2005448c0024c4c4e6fb3a3673570ee348",
    "08a12e51872ba51b24aeddbae550779490d81178a042330203010001"
);

const SERVER_CERTIFICATE_DER_HEX: &str = concat!(
    "30820309308201f1a003020102021442e4d928a0006a8460e53e76862c364294475ea0300d06",
    "092a864886f70d01010b050030143112301006035504030c096c6f63616c686f7374301e170d",
    "3236303530383034343733365a170d3236303530393034343733365a30143112301006035504",
    "030c096c6f63616c686f737430820122300d06092a864886f70d01010105000382010f003082",
    "010a0282010100ce62e4d7c07c9f5fde5df7d0e7f8782fd30092b6cb8b4ecece7c0e46093db39",
    "dfb3069c58575d2b9890de744ccab5a3cf8d1c08f4b39d3a327be4f1e011f7f84672c78b74d67",
    "c417a45e30d7db6ac3893115e1b0531fa763a315415503c20549670baf7d5dca4dad09ac1310",
    "a5fea4dec49668efcbb731aaa8f954ea1ef6aa792e45c6283be60ba0206e8b61572307f342ee",
    "f77eeeb4fd45094ace6f282c9e49fb0f38dd9078383a8e393e594e048512d7960efbee4efbce7",
    "11f115fcfb265eeb057871e0b41ed38b9fa06a62b248295d2a58b2a980c6bcd2005448c0024c",
    "4c4e6fb3a3673570ee34808a12e51872ba51b24aeddbae550779490d81178a0423302030100",
    "01a3533051301d0603551d0e041604144ec28ad6f4ad832e17819513a7c587cf2c577f96301f",
    "0603551d230418301680144ec28ad6f4ad832e17819513a7c587cf2c577f96300f0603551d13",
    "0101ff040530030101ff300d06092a864886f70d01010b05000382010100a514d79be510569c",
    "ba726916373fde639d5b8fe8fca8de76d8afdc2d5c63a19060c9a03bd6055fc8d884fc60caadd",
    "a64a85ed39e0df28160e78a91d552fc1f82cc489f237890b1a65f419152f2e04dfd30a38930b",
    "76c7a44fc1859e63d0d393f4f2399eea53ce238705eb30772663cabdd4ae83f1b79ba7e8b491",
    "8542ab4b13890b8c9e6eb4e1e350253a9171ee9b3bcfc5ca787a7e2d97bfb6e726d2cb36a4a28",
    "3e13eacdcd7ca35756aab28e5a0fb11b5be61e37ccda157ee424272b825a626eac6415276472",
    "10a794c3ca3714f1162e3184c059d9198156197965b6422ce8cc29576be2bbdaf5cecffed833",
    "ee8c15e629f5c4e0331f3af7cfaf99b7985bdd"
);

fn decode_hex(hex: &str) -> Vec<u8> {
    hex::decode(hex).expect("valid hex fixture")
}

pub fn server_certificate_der() -> Vec<u8> {
    static CERT: OnceLock<Vec<u8>> = OnceLock::new();
    CERT.get_or_init(|| decode_hex(SERVER_CERTIFICATE_DER_HEX))
        .clone()
}

pub fn server_public_key_der() -> Vec<u8> {
    static KEY: OnceLock<Vec<u8>> = OnceLock::new();
    KEY.get_or_init(|| decode_hex(SERVER_PUBLIC_KEY_DER_HEX))
        .clone()
}

pub fn server_private_key() -> RsaPrivateKey {
    static KEY: OnceLock<Vec<u8>> = OnceLock::new();
    let der = KEY.get_or_init(|| decode_hex(SERVER_PRIVATE_KEY_DER_HEX));
    RsaPrivateKey::from_pkcs8_der(der).expect("valid RSA private key fixture")
}

pub fn build_server_hello_record(
    server_random: [u8; 32],
    session_id: [u8; 32],
    cipher_suite: u16,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0x0302_u16.to_be_bytes());
    body.extend_from_slice(&server_random);
    body.push(session_id.len() as u8);
    body.extend_from_slice(&session_id);
    body.extend_from_slice(&cipher_suite.to_be_bytes());
    body.push(0);
    body.extend_from_slice(&0_u16.to_be_bytes());

    let mut server_hello = vec![2];
    server_hello.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
    server_hello.extend_from_slice(&body);

    let mut record = vec![22];
    record.extend_from_slice(&0x0302_u16.to_be_bytes());
    record.extend_from_slice(&(server_hello.len() as u16).to_be_bytes());
    record.extend_from_slice(&server_hello);
    record
}

pub fn build_server_flight_record(
    server_random: [u8; 32],
    session_id: [u8; 32],
    cert_der: &[u8],
    cipher_suite: u16,
) -> Vec<u8> {
    let server_hello = build_server_hello_record(server_random, session_id, cipher_suite);
    let handshake = &server_hello[5..];

    let mut cert_list = Vec::new();
    cert_list.extend_from_slice(&(cert_der.len() as u32).to_be_bytes()[1..4]);
    cert_list.extend_from_slice(cert_der);

    let mut cert_body = Vec::new();
    cert_body.extend_from_slice(&(cert_list.len() as u32).to_be_bytes()[1..4]);
    cert_body.extend_from_slice(&cert_list);

    let mut certificate = vec![11];
    certificate.extend_from_slice(&(cert_body.len() as u32).to_be_bytes()[1..4]);
    certificate.extend_from_slice(&cert_body);

    let payload = [handshake.to_vec(), certificate, vec![14, 0, 0, 0]].concat();
    let mut record = vec![22];
    record.extend_from_slice(&0x0302_u16.to_be_bytes());
    record.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    record.extend_from_slice(&payload);
    record
}

pub fn decrypt_client_key_exchange(handshake: &[u8], private_key: &RsaPrivateKey) -> [u8; 48] {
    let encrypted_len = u16::from_be_bytes([handshake[4], handshake[5]]) as usize;
    let encrypted = &handshake[6..6 + encrypted_len];
    let decrypted = private_key
        .decrypt(Pkcs1v15Encrypt, encrypted)
        .expect("decrypt client key exchange");
    let mut out = [0_u8; 48];
    out.copy_from_slice(&decrypted);
    out
}
