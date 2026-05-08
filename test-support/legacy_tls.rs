use std::sync::OnceLock;

use rsa::RsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Encrypt;
use rsa::pkcs8::DecodePrivateKey;

const SERVER_PRIVATE_KEY_DER_HEX: &str = concat!(
    "308204bc020100300d06092a864886f70d0101010500048204a6308204a20201000282010100",
    "b7d5a42bd80774d558decb5cd00afa9740e7a5e2b8d2c6ef75dab95035f46550fb1a6397d6d8",
    "eac7a9bcb84c25ad44f0c0b3031ffab0e700e66d6b4ede07a1780a59714862f18e0acf1461bb",
    "fa2a0f525cd5c96294303b02ff8b88cb1588369aff64b02a180de558d665eaed0e6cbdf73a49",
    "11fc0427f0cd7e544215cc848b160ad1c2965ffee180203aaf131dcf0547df37acf70560ab44",
    "e5b306b60d5094f22ef150130daae09544e45d8ed710545362109a7bb85cee6fb0882944dc15",
    "c62d53a0f68ca4ba81ece9d4630fedd829837c9056ff149ac0f257329cf6fd4c04c77aaddcb8",
    "39dd1d20ada9137c9b3b7ff6ce468caa32f02cf80ef63783de1b5f2b02030100010282010036",
    "00b87d78b49809a455ae7fd14da8578e657e419ff1ab26e5041fb404658aedc315f973bd5573",
    "82bbc6088db5f3b470d4eac15c3f948007afa92d00ba109bca5b9ff2bb44f598b86c249ca14f",
    "d7d3306abd12afb6c0845559247046d8486f6210ec4f23ce75268e764cf23a3926909773b3cb",
    "68b0ffdaa943171562b9f5a5b59068c2347c30c96deb9456f4d0fb151914b956359d96c64309",
    "534d6d8777ae4027b5134c7d4bc5ca71fe07266c69bf94bd43ba3a8b7a0e62e1aa3b00bcaa96",
    "31f65ea5ca69d0db8dac19620afd955d04c4c0103913dae35c38e1f662c818066132512ae2c1",
    "043fc10c888b61fe610ae35e05308dafc99eb81522d7df11c4972102818100e065d25cdb1b7c",
    "1474ebdf0088b1e862898cefe6a92d1ef1a5acd34fb24dfd019eaaa4b912f08a0f172caf8c33",
    "20008a1f19264f3685dd9dea3437f781da584e296b4dbc0f5ac95468fbacc511490fd38466aa",
    "e66f9f08755b1093457a944d6d13d3d1e645c72472fda94f915a49953001012d8f185340541b",
    "f86ca55329a34702818100d1b96680483c4e9e9c7020f546a92f9027850aa0b7f29b2413a21d",
    "798b80aeb4f72e5265f067da82abc51177e017f410ca3882d2e9ec6c4ae2d293cf238266194b",
    "128d687f4f5faecb52f73b483599f39a45cbe366a55a0930485808043fd90f13943defa2667d",
    "5e5344dbfade2f70b3a4413042d4f1ba5a9a646f32e10feefd0281807338752a8eaaef2c45d8",
    "f3397ff6f1dedec80a4ac2e553353b1fa1a51365ee1a8dd10b21a8b2f000cda2794520df36c7",
    "b52e21b89cc361c6fb01f316d88b37ba7294116715ef44df5dc494b2dfac473f1185f20cbe95",
    "c72f132250ac65438efa0a2b4264ddc1625ad51ac1ab5dd3d891bc8251555f6527ebc82ce804",
    "7fce2a730281804646219a8a99ea9e2b3d474de8c778308d8d7eea2bb917531761130f4f3767",
    "532c20516be70a65a5b378ed4985c580fabf48004e3c240485734bac4b94527573b43c1b3610",
    "b2c01509fc5aeb7a5ebb9f6cea464f846d93a5d08fed2f827d027692d0f1900292e51f5e378c",
    "9e9b24ba56f78b0068e481bb37f0d4068ebdeb60e1028180570a91b04c05393c5123f6aa45b1",
    "a528536d99dde8771b0954796a81f8e4fb0e445cf1af73cf509000f162a82a1a531efe490520",
    "f8b2e3ba6d6297a1d82761d4cf7f7d3198b167dc17c2266f459a72dd1e82083971bccb8f68e1",
    "010c6d569caead78360e33e8a0f75f5bb6883277e30516d14edadcc879c0f35cbb15b827e4e4"
);

const SERVER_PUBLIC_KEY_DER_HEX: &str = concat!(
    "30820122300d06092a864886f70d01010105000382010f003082010a0282010100b7d5a42bd8",
    "0774d558decb5cd00afa9740e7a5e2b8d2c6ef75dab95035f46550fb1a6397d6d8eac7a9bcb8",
    "4c25ad44f0c0b3031ffab0e700e66d6b4ede07a1780a59714862f18e0acf1461bbfa2a0f525c",
    "d5c96294303b02ff8b88cb1588369aff64b02a180de558d665eaed0e6cbdf73a4911fc0427f0",
    "cd7e544215cc848b160ad1c2965ffee180203aaf131dcf0547df37acf70560ab44e5b306b60d",
    "5094f22ef150130daae09544e45d8ed710545362109a7bb85cee6fb0882944dc15c62d53a0f6",
    "8ca4ba81ece9d4630fedd829837c9056ff149ac0f257329cf6fd4c04c77aaddcb839dd1d20ad",
    "a9137c9b3b7ff6ce468caa32f02cf80ef63783de1b5f2b0203010001"
);

const SERVER_CERTIFICATE_DER_HEX: &str = concat!(
    "3082035030820238a00302010202145b22c0d030a16bdd9a73b026e59cf228fd1c1bf2300d06",
    "092a864886f70d01010b0500301e311c301a06035504030c13536d656c6c7920546573742052",
    "6f6f74204341301e170d3236303530383035353631355a170d3336303530353035353631355a",
    "30143112301006035504030c096c6f63616c686f737430820122300d06092a864886f70d0101",
    "0105000382010f003082010a0282010100b7d5a42bd80774d558decb5cd00afa9740e7a5e2b8",
    "d2c6ef75dab95035f46550fb1a6397d6d8eac7a9bcb84c25ad44f0c0b3031ffab0e700e66d6b",
    "4ede07a1780a59714862f18e0acf1461bbfa2a0f525cd5c96294303b02ff8b88cb1588369aff",
    "64b02a180de558d665eaed0e6cbdf73a4911fc0427f0cd7e544215cc848b160ad1c2965ffee1",
    "80203aaf131dcf0547df37acf70560ab44e5b306b60d5094f22ef150130daae09544e45d8ed7",
    "10545362109a7bb85cee6fb0882944dc15c62d53a0f68ca4ba81ece9d4630fedd829837c9056",
    "ff149ac0f257329cf6fd4c04c77aaddcb839dd1d20ada9137c9b3b7ff6ce468caa32f02cf80e",
    "f63783de1b5f2b0203010001a3818f30818c301a0603551d110413301182096c6f63616c686f",
    "737487047f00000130090603551d1304023000300e0603551d0f0101ff0404030205a0301306",
    "03551d25040c300a06082b06010505070301301d0603551d0e041604145649e1d290ffab0d2e",
    "87e1882fa6a41b58fb1094301f0603551d230418301680145943e8daed9c255662ce26668397",
    "21959012124d300d06092a864886f70d01010b0500038201010048d6b0b09979830407888ead",
    "cb2907f75ac6121f59aa3743d4852caf5a94f92352e4ed325f3b4719c88ffe3b1af0b9e72075",
    "b96d6e0ae12592447d819c4f1a399f243d267f872019ef745a0af7d69f3842daf1dfefdd57c5",
    "0665772dcc09a8205155a90f466d2043ed4e85f135a21bb408141caf5bded63917b2f3bf47ad",
    "7623842fcbe88fe8853d9e6e58d078f9d1cf8944e4866f806c22f625cf912d81647e34d40682",
    "a8908a7de825f74c7b53c3fcd200ff82d07ab808156f9c82ea6a0c0b5af5b5641b33a0e21986",
    "2062aaf6ddfbde2bc9c118e5b4c081e8caff0d644862ba3a226cdc64bfaba44f68fa074eaf9f",
    "f47b252d04b134a066f4dd2f60312baa"
);

const ROOT_CERTIFICATE_DER_HEX: &str = concat!(
    "3082032d30820215a00302010202146656668e8bcd407c4621f488c259682e3725af77300d06",
    "092a864886f70d01010b0500301e311c301a06035504030c13536d656c6c7920546573742052",
    "6f6f74204341301e170d3236303530383035353631355a170d3336303530353035353631355a",
    "301e311c301a06035504030c13536d656c6c79205465737420526f6f7420434130820122300d",
    "06092a864886f70d01010105000382010f003082010a0282010100a95bab013f095f3fcf5763",
    "3aa97752b76bf2c2477097a8d85b62b6a66cfc0c5b04bb61c72df384ff01d92232d43e179dd5",
    "19a3b9b54df7a71accc47ab469201dcb92938ad15469371ef6c0010677fc0cae562539ad4a3e",
    "5cc278cba9f7e2375fee2b677ded6fbd412eea4a5c141358fdec9a14eaea100663973e6881c8",
    "e02a98f799105d2eeeb10b008ad14b52e8ea1b6918e877ccdbba8fb426bb698749c81e48c223",
    "286afd631e4aadccab86452d819c118390b4d9a3401a20c3a732bf69d5f052e506ad6c99a113",
    "c73f72103dc4354034c97f9a3400811320223c08ea7549af102d29b6b293c2f80a47e6fbc849",
    "c9d61ed6e56f3b0babb2c828f198e35fd70203010001a3633061301d0603551d0e0416041459",
    "43e8daed9c255662ce2666839721959012124d301f0603551d230418301680145943e8daed9c",
    "255662ce2666839721959012124d300f0603551d130101ff040530030101ff300e0603551d0f",
    "0101ff040403020106300d06092a864886f70d01010b050003820101009c17a775d34d81cf34",
    "e13b0052bae808736b36f9c7f412767af7f5988f342845079ed929b782e08be6174d27d2c55a",
    "50d0c6a1a3fee4db0ae5e60f811c8372d272c3684827ade11c43662c1d394a8825e813cedab6",
    "c1c4b6c64a890229b331e45d182f563c32ada7276471ee88817c3a82422ad5b0cdc4ebeadc1b",
    "6e016fc5946b139c13728767e2caa8546daa5a428265355652e17c17f37872c06983e30ef44c",
    "bf45229831ffa023c6bfa1ec727524de7d0c7d68914ea8e605a11e459ac855e6526e7fa4f80b",
    "8e51fd21d70bd2bb15c0e3301adde0bb0de239e95c99d8bbdabe42995a8dc9b223692f1e7958",
    "6b1076aa099e175cf04f42e7d127d01a2091eb"
);

const ALTERNATE_ROOT_CERTIFICATE_DER_HEX: &str = concat!(
    "3082033130820219a00302010202141a5a7e6170033407d56eb82c1c1b030b2fb105d9300d06",
    "092a864886f70d01010b050030283126302406035504030c1d536d656c6c7920416c7465726e",
    "617465205465737420526f6f74204341301e170d3236303530383036343833305a170d333630",
    "3530353036343833305a30283126302406035504030c1d536d656c6c7920416c7465726e6174",
    "65205465737420526f6f7420434130820122300d06092a864886f70d01010105000382010f00",
    "3082010a0282010100b112112e158c58c8641b4b8aa0b0b74ad9e65a3693cbae863bdc7a2d8f",
    "a1364936ce5037bdb64a84a2fef7913a1902bf88a9d614d770345e75db11c2114c5b7eb10e54",
    "cafe0fc70943987924a5b868c53d48e0e0f06579b5781a98c0926c6e56c31cad65b61ef64b53",
    "d664e92de4e678db8e05aa8ea3d9611c71d1e129c89b14b9b586211d5a74bc9c1958021b6a8c",
    "be82a8de00e51d90dcf09f88e8199065d3b5ee1ead1f7993b4388534b8616dd6d424637c51f4",
    "ddc1b5cc4196ac477f60cc45b40bec639fe2226778ed626c30eb7c626123399cf40861ffa5bc",
    "c3c92863d2c136126fd9d9ee88eac00d1cce237409c7f9a227d27660e6e5e4bbe31b0bd9b702",
    "03010001a3533051301d0603551d0e04160414df6a00c52603aefe42dce554b99f46eee5e1f4",
    "ef301f0603551d23041830168014df6a00c52603aefe42dce554b99f46eee5e1f4ef300f0603",
    "551d130101ff040530030101ff300d06092a864886f70d01010b050003820101004238f42b5f",
    "309540e886b3ca1ba20c2a5e2fc03d6d39d1817ba04d3fa4c9d2ba1dd3f732f8a6c8b3e368fa",
    "ac030e4fea34da13c713052f5b3fee0cde1b02a176458762cff3ab694daaf8278fbaaf3db221",
    "d9d23073962388c2f499d41fd2d698f0d35a97e1e235d102dad12cbf0b723d10429ba46e19c5",
    "8ff442ee5f597f2a5d6cb2bb6f8558d41485e12ad8d9b08ad6286e4459f2205ffec025ce94c1",
    "dd73bf537cd849402ac9f59fb46f3f58c78ff42c24fa9f5a033c3e2f0632a6b0414d38090584",
    "637e7d1697562c6971af29d8a693156cceee1065b2b5fbf994f2a63dfdf89cf9590c8e588f1c",
    "6894517bf5264fb157525f1fa2f9a6559434604a470f48"
);

fn decode_hex(hex: &str) -> Vec<u8> {
    hex::decode(hex).expect("valid hex fixture")
}

pub fn server_certificate_der() -> Vec<u8> {
    static CERT: OnceLock<Vec<u8>> = OnceLock::new();
    CERT.get_or_init(|| decode_hex(SERVER_CERTIFICATE_DER_HEX))
        .clone()
}

pub fn root_certificate_der() -> Vec<u8> {
    static CERT: OnceLock<Vec<u8>> = OnceLock::new();
    CERT.get_or_init(|| decode_hex(ROOT_CERTIFICATE_DER_HEX))
        .clone()
}

pub fn alternate_root_certificate_der() -> Vec<u8> {
    static CERT: OnceLock<Vec<u8>> = OnceLock::new();
    CERT.get_or_init(|| decode_hex(ALTERNATE_ROOT_CERTIFICATE_DER_HEX))
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

pub fn server_private_key_der() -> Vec<u8> {
    static KEY: OnceLock<Vec<u8>> = OnceLock::new();
    KEY.get_or_init(|| decode_hex(SERVER_PRIVATE_KEY_DER_HEX))
        .clone()
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
