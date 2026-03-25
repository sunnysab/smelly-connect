use crate::error::ProtocolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedToken(pub [u8; 48]);

impl DerivedToken {
    pub fn as_bytes(&self) -> &[u8; 48] {
        &self.0
    }
}

pub fn derive_token(
    server_session_id_hex: &str,
    twfid: &str,
) -> Result<DerivedToken, ProtocolError> {
    if server_session_id_hex.len() < 31 {
        return Err(ProtocolError::InvalidSessionIdLength);
    }

    let token = format!("{}\0{twfid}", &server_session_id_hex[..31]);
    let token_bytes = token.as_bytes();
    if token_bytes.len() != 48 {
        return Err(ProtocolError::InvalidSessionIdLength);
    }

    let mut derived = [0_u8; 48];
    derived.copy_from_slice(token_bytes);
    Ok(DerivedToken(derived))
}
