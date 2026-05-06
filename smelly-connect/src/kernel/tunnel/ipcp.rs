use super::rc4::RC4State;

/// IPCP frame header size.
pub const IPCP_HEADER_LEN: usize = 10;

/// IPCP record type byte.
const RECORD_TYPE: u8 = 0x17;
const VERSION_MAJOR: u8 = 0x03;
const VERSION_MINOR: u8 = 0x01;

/// Flag: payload is RC4-encrypted.
const FLAG_ENCRYPTED: u8 = 0x80;
/// Mask for compression method in low 7 bits.
const COMPRESS_MASK: u8 = 0x7f;

/// Compression methods.
pub const COMPRESS_NONE: u8 = 0;
pub const COMPRESS_LZO: u8 = 3;
pub const COMPRESS_ZLIB: u8 = 5;

/// Encode an IP packet into an IPCP frame.
///
/// Order: compress → encrypt → frame header.
/// `enc_type`: 0 = RC4 encryption enabled.
/// `zip_flag`: compression method (0=none, 3=LZO, 5=ZLIB).
/// `rc4`: mutable RC4 state for encryption (only used when enc_type == 0).
pub fn encode_ipcp(
    payload: &[u8],
    enc_type: u32,
    zip_flag: u32,
    rc4: &mut RC4State,
) -> Vec<u8> {
    let (body, used_compress) = match zip_flag {
        3 if payload.len() >= 200 => {
            // LZO compression — not implemented, pass through
            (payload.to_vec(), COMPRESS_NONE)
        }
        5 if payload.len() >= 200 => {
            // ZLIB compression — not implemented, pass through
            (payload.to_vec(), COMPRESS_NONE)
        }
        _ => (payload.to_vec(), COMPRESS_NONE),
    };

    let mut body = body;
    let mut flags = used_compress;

    if enc_type == 0 {
        let plain = body.clone();
        rc4.xor_key_stream(&mut body, &plain);
        flags |= FLAG_ENCRYPTED;
    }

    let total_len = (body.len() + 5) as u16;
    let orig_len = payload.len() as u16;

    let mut frame = Vec::with_capacity(IPCP_HEADER_LEN + body.len());
    frame.push(RECORD_TYPE);
    frame.push(VERSION_MAJOR);
    frame.push(VERSION_MINOR);
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.push(flags);
    frame.push(0); // reserved
    frame.push(0); // reserved
    frame.extend_from_slice(&orig_len.to_be_bytes());
    frame.extend_from_slice(&body);
    frame
}

/// Decode an IPCP frame into an IP packet.
///
/// The frame must start at offset 0. Returns the decoded payload and the
/// number of bytes consumed from the input.
///
/// Order: deframe → decrypt → decompress.
pub fn decode_ipcp(
    frame: &[u8],
    rc4: &mut RC4State,
) -> Result<Vec<u8>, IPCPError> {
    if frame.len() < IPCP_HEADER_LEN {
        return Err(IPCPError::ShortFrame {
            got: frame.len(),
            min: IPCP_HEADER_LEN,
        });
    }
    if frame[0] != RECORD_TYPE || frame[1] != VERSION_MAJOR || frame[2] != VERSION_MINOR {
        return Err(IPCPError::BadPrefix {
            got: [frame[0], frame[1], frame[2]],
        });
    }

    let body_len = u16::from_be_bytes(frame[3..5].try_into().unwrap()) as usize;
    if body_len < 5 {
        return Err(IPCPError::BadLength { body_len });
    }
    let payload_len = body_len - 5;
    if frame.len() < IPCP_HEADER_LEN + payload_len {
        return Err(IPCPError::ShortFrame {
            got: frame.len(),
            min: IPCP_HEADER_LEN + payload_len,
        });
    }

    let flags = frame[5];
    let orig_len = u16::from_be_bytes(frame[8..10].try_into().unwrap()) as usize;
    let mut body = frame[IPCP_HEADER_LEN..IPCP_HEADER_LEN + payload_len].to_vec();

    // Decrypt if encrypted flag is set
    if flags & FLAG_ENCRYPTED != 0 {
        let plain = body.clone();
        rc4.xor_key_stream(&mut body, &plain);
    }

    // Decompress
    let method = flags & COMPRESS_MASK;
    match method {
        COMPRESS_NONE => {}
        COMPRESS_LZO => return Err(IPCPError::UnsupportedCompression(method)),
        COMPRESS_ZLIB => return Err(IPCPError::UnsupportedCompression(method)),
        _ => return Err(IPCPError::UnsupportedCompression(method)),
    }

    if body.len() != orig_len {
        return Err(IPCPError::LengthMismatch {
            decoded: body.len(),
            expected: orig_len,
        });
    }

    Ok(body)
}

/// Read one complete IPCP frame from a reader.
/// Returns (decoded_payload, total_bytes_consumed).
pub fn read_ipcp_frame<R: std::io::Read>(
    reader: &mut R,
    rc4: &mut RC4State,
) -> Result<Vec<u8>, IPCPError> {
    let mut hdr = [0u8; IPCP_HEADER_LEN];
    reader.read_exact(&mut hdr).map_err(IPCPError::Io)?;

    if hdr[0] != RECORD_TYPE || hdr[1] != VERSION_MAJOR || hdr[2] != VERSION_MINOR {
        return Err(IPCPError::BadPrefix {
            got: [hdr[0], hdr[1], hdr[2]],
        });
    }

    let body_len = u16::from_be_bytes(hdr[3..5].try_into().unwrap()) as usize;
    if body_len < 5 {
        return Err(IPCPError::BadLength { body_len });
    }
    let payload_len = body_len - 5;

    let mut body = vec![0u8; payload_len];
    reader.read_exact(&mut body).map_err(IPCPError::Io)?;

    let flags = hdr[5];
    let orig_len = u16::from_be_bytes(hdr[8..10].try_into().unwrap()) as usize;

    if flags & FLAG_ENCRYPTED != 0 {
        let plain = body.clone();
        rc4.xor_key_stream(&mut body, &plain);
    }

    let method = flags & COMPRESS_MASK;
    match method {
        COMPRESS_NONE => {}
        _ => return Err(IPCPError::UnsupportedCompression(method)),
    }

    if body.len() != orig_len {
        return Err(IPCPError::LengthMismatch {
            decoded: body.len(),
            expected: orig_len,
        });
    }

    Ok(body)
}

#[derive(Debug)]
pub enum IPCPError {
    ShortFrame { got: usize, min: usize },
    BadPrefix { got: [u8; 3] },
    BadLength { body_len: usize },
    UnsupportedCompression(u8),
    LengthMismatch { decoded: usize, expected: usize },
    Io(std::io::Error),
}

impl std::fmt::Display for IPCPError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShortFrame { got, min } => {
                write!(f, "short IPCP frame: {got} bytes, need at least {min}")
            }
            Self::BadPrefix { got } => {
                write!(f, "bad IPCP prefix: {:02x?}", got)
            }
            Self::BadLength { body_len } => {
                write!(f, "bad IPCP body length: {body_len}")
            }
            Self::UnsupportedCompression(m) => {
                write!(f, "unsupported compression method: {m}")
            }
            Self::LengthMismatch { decoded, expected } => {
                write!(f, "decoded length {decoded} != expected {expected}")
            }
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for IPCPError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipcp_roundtrip_no_encrypt() {
        let key = b"0123456789abcdef";
        let original = vec![0x45, 0x00, 0x00, 0x1c, 0x00, 0x01, 0x00, 0x00, 0x40, 0x06];
        let mut enc_rc4 = RC4State::new(key);
        let mut dec_rc4 = RC4State::new(key);

        let frame = encode_ipcp(&original, 1, 0, &mut enc_rc4); // enc_type=1 means no encrypt
        let decoded = decode_ipcp(&frame, &mut dec_rc4).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn ipcp_roundtrip_rc4_encrypt() {
        let key = b"0123456789abcdef";
        let original = vec![0x45, 0x00, 0x00, 0x1c, 0x00, 0x01, 0x00, 0x00, 0x40, 0x06];
        let mut enc_rc4 = RC4State::new(key);
        let mut dec_rc4 = RC4State::new(key);

        let frame = encode_ipcp(&original, 0, 0, &mut enc_rc4); // enc_type=0 = RC4
        // Frame should be encrypted (flags & 0x80)
        assert_ne!(&frame[10..], &original);
        let decoded = decode_ipcp(&frame, &mut dec_rc4).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn ipcp_header_format() {
        let key = b"0123456789abcdef";
        let payload = vec![0xAA; 100];
        let mut rc4 = RC4State::new(key);
        let frame = encode_ipcp(&payload, 0, 0, &mut rc4);

        assert_eq!(frame[0], 0x17);
        assert_eq!(frame[1], 0x03);
        assert_eq!(frame[2], 0x01);
        let total_len = u16::from_be_bytes(frame[3..5].try_into().unwrap());
        assert_eq!(total_len as usize, 100 + 5);
        assert_eq!(frame[5], 0x80); // encrypted flag
        let orig_len = u16::from_be_bytes(frame[8..10].try_into().unwrap());
        assert_eq!(orig_len as usize, 100);
    }

    #[test]
    fn ipcp_short_frame_error() {
        let key = b"0123456789abcdef";
        let mut rc4 = RC4State::new(key);
        assert!(matches!(
            decode_ipcp(&[0x17, 0x03], &mut rc4),
            Err(IPCPError::ShortFrame { .. })
        ));
    }

    #[test]
    fn ipcp_bad_prefix_error() {
        let key = b"0123456789abcdef";
        let mut rc4 = RC4State::new(key);
        let frame = vec![0x00, 0x03, 0x01, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(matches!(
            decode_ipcp(&frame, &mut rc4),
            Err(IPCPError::BadPrefix { .. })
        ));
    }
}
