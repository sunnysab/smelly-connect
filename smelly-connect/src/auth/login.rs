#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAuthResponse {
    pub twfid: String,
    pub rsa_key_hex: String,
    pub rsa_exp: u32,
    pub csrf_rand_code: Option<String>,
    pub requires_captcha: bool,
}

pub fn parse_login_auth(body: &str) -> Result<LoginAuthResponse, ParseLoginAuthError> {
    let twfid = extract_tag(body, "TwfID").ok_or(ParseLoginAuthError::MissingTag("TwfID"))?;
    let rsa_key_hex = extract_tag(body, "RSA_ENCRYPT_KEY")
        .ok_or(ParseLoginAuthError::MissingTag("RSA_ENCRYPT_KEY"))?;
    let rsa_exp = extract_tag(body, "RSA_ENCRYPT_EXP")
        .unwrap_or("65537")
        .parse()
        .map_err(|_| ParseLoginAuthError::InvalidRsaExponent)?;
    let csrf_rand_code = extract_tag(body, "CSRF_RAND_CODE").map(ToOwned::to_owned);
    let requires_captcha = extract_tag(body, "RndImg") == Some("1");

    Ok(LoginAuthResponse {
        twfid: twfid.to_owned(),
        rsa_key_hex: rsa_key_hex.to_owned(),
        rsa_exp,
        csrf_rand_code,
        requires_captcha,
    })
}

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

fn extract_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = body.find(&start_tag)? + start_tag.len();
    let end = body[start..].find(&end_tag)? + start;
    Some(body[start..end].trim())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseLoginAuthError {
    MissingTag(&'static str),
    InvalidRsaExponent,
}
use hex::encode;
use rsa::BigUint;
use rsa::RsaPublicKey;
use rsa::pkcs1v15::Pkcs1v15Encrypt;

use crate::error::AuthError;
