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

/// Metadata parsed from `/por/conf.csp` response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfMetadata {
    pub login_name: Option<String>,
    pub is_relogin: Option<String>,
    pub svpn_id: Option<String>,
    pub mline_enable: bool,
    pub mline_list: Vec<String>,
}
