pub const DEFAULT_LEGACY_CIPHER_SUITE: u16 = smelly_tls::TLS_RSA_WITH_RC4_128_SHA;

pub fn cipher_suite_attempts(hint: Option<&str>) -> Vec<u16> {
    let preferred = hint
        .and_then(smelly_tls::legacy_cipher_suite_from_hint)
        .unwrap_or(DEFAULT_LEGACY_CIPHER_SUITE);
    let mut attempts = vec![preferred];
    if preferred != DEFAULT_LEGACY_CIPHER_SUITE {
        attempts.push(DEFAULT_LEGACY_CIPHER_SUITE);
    }
    attempts
}
