pub fn cipher_suite_attempts(hint: Option<&str>) -> Vec<u16> {
    smelly_tls::easyconnect_cipher_suite_attempts(
        hint.and_then(smelly_tls::legacy_cipher_suite_from_hint),
    )
}
