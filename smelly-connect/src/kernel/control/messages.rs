use crate::resource::ResourceSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAuthChallenge {
    pub twfid: String,
    pub rsa_key_hex: String,
    pub rsa_exp: u32,
    pub csrf_rand_code: Option<String>,
    pub legacy_cipher_hint: Option<String>,
    pub requires_captcha: bool,
}

pub type ResourceDocument = ResourceSet;
