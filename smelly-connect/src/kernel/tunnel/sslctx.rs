#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedSSLContext {
    pub raw: [u8; 64],
}

impl DecodedSSLContext {
    pub fn randnum(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out.copy_from_slice(&self.raw[0x20..0x30]);
        out
    }

    pub fn key(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out.copy_from_slice(&self.raw[0x30..0x40]);
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SSLContextError {
    InvalidHexLength(usize),
    HexDecode(hex::FromHexError),
}

impl From<hex::FromHexError> for SSLContextError {
    fn from(e: hex::FromHexError) -> Self {
        Self::HexDecode(e)
    }
}

pub fn decode_sslctx_hex(hex_str: &str) -> Result<DecodedSSLContext, SSLContextError> {
    if hex_str.len() != 128 {
        return Err(SSLContextError::InvalidHexLength(hex_str.len()));
    }
    let bytes = hex::decode(hex_str)?;
    let mut raw = [0u8; 64];
    raw.copy_from_slice(&bytes);
    Ok(DecodedSSLContext { raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_valid_sslctx() {
        // 32-byte prefix + 16-byte randnum + 16-byte key = 64 bytes = 128 hex chars
        let prefix = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let randnum = "aabbccdd00112233aabbccdd00112233";
        let key = "44556677889900aa44556677889900aa";
        let hex_str = format!("{prefix}{randnum}{key}");
        assert_eq!(hex_str.len(), 128);

        let decoded = decode_sslctx_hex(&hex_str).unwrap();
        assert_eq!(&decoded.raw[0x00..0x20], &hex::decode(prefix).unwrap());
        assert_eq!(&decoded.raw[0x20..0x30], &hex::decode(randnum).unwrap());
        assert_eq!(&decoded.raw[0x30..0x40], &hex::decode(key).unwrap());
        assert_eq!(decoded.randnum(), hex::decode(randnum).unwrap().as_slice());
        assert_eq!(decoded.key(), hex::decode(key).unwrap().as_slice());
    }

    #[test]
    fn reject_wrong_length() {
        assert!(matches!(
            decode_sslctx_hex("aabb"),
            Err(SSLContextError::InvalidHexLength(4))
        ));
    }

    #[test]
    fn reject_invalid_hex() {
        let bad = "z".repeat(128);
        assert!(matches!(
            decode_sslctx_hex(&bad),
            Err(SSLContextError::HexDecode(_))
        ));
    }
}
