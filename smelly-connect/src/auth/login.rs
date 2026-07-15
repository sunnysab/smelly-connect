pub use crate::kernel::control::{
    ControlParseError as ParseLoginAuthError, LoginAuthChallenge as LoginAuthResponse,
    parse_login_auth_challenge as parse_login_auth,
};

pub fn encrypt_password(
    password: &str,
    csrf_rand_code: Option<&str>,
    rsa_key_hex: &str,
    rsa_exp: u32,
) -> Result<String, AuthError> {
    let payload = match csrf_rand_code {
        Some(csrf) => format!("{password}_{csrf}"),
        None => password.to_string(),
    };

    let modulus =
        BigUint::parse_bytes(rsa_key_hex.as_bytes(), 16).ok_or(AuthError::InvalidModulusHex)?;
    let public_key = RsaPublicKey::new(modulus, BigUint::from(rsa_exp))
        .map_err(|_| AuthError::InvalidPublicExponent)?;

    let mut rng = rsa::rand_core::OsRng;
    let encrypted = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, payload.as_bytes())
        .map_err(|_| AuthError::EncryptFailed)?;

    Ok(encode(encrypted))
}

use hex::encode;
use rsa::BigUint;
use rsa::RsaPublicKey;
use rsa::pkcs1v15::Pkcs1v15Encrypt;

use crate::error::AuthError;
