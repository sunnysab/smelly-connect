/// Standard RC4 implementation (KSA + PRGA).
#[derive(Debug, Clone)]
pub struct RC4State {
    x: u8,
    y: u8,
    s: [u8; 256],
}

impl RC4State {
    pub fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for i in 0..256 {
            s[i] = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Self { x: 0, y: 0, s }
    }

    /// XOR `src` with RC4 keystream, writing result to `dst`.
    pub fn xor_key_stream(&mut self, dst: &mut [u8], src: &[u8]) {
        for i in 0..src.len() {
            self.x = self.x.wrapping_add(1);
            self.y = self.y.wrapping_add(self.s[self.x as usize]);
            self.s.swap(self.x as usize, self.y as usize);
            let k = self.s[(self.s[self.x as usize] as u16 + self.s[self.y as usize] as u16) as usize & 0xff];
            dst[i] = src[i] ^ k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_test_vector_key_12345() {
        // RC4("Key", "Plaintext") = BBF316E8D940AF0AD3
        let mut state = RC4State::new(b"Key");
        let plaintext = b"Plaintext";
        let mut output = vec![0u8; plaintext.len()];
        state.xor_key_stream(&mut output, plaintext);
        let expected = [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3];
        assert_eq!(output, expected);
    }

    #[test]
    fn rc4_test_vector_wiki() {
        // Wikipedia example: RC4("Wiki", "pedia") = 1021BF0420
        let mut state = RC4State::new(b"Wiki");
        let plaintext = b"pedia";
        let mut output = vec![0u8; plaintext.len()];
        state.xor_key_stream(&mut output, plaintext);
        let expected = [0x10, 0x21, 0xBF, 0x04, 0x20];
        assert_eq!(output, expected);
    }

    #[test]
    fn rc4_roundtrip() {
        let key = b"supersecretkey12";
        let original = b"Hello, EasyConnect VPN tunnel data!";
        let mut encrypt_state = RC4State::new(key);
        let mut encrypted = vec![0u8; original.len()];
        encrypt_state.xor_key_stream(&mut encrypted, original);

        let mut decrypt_state = RC4State::new(key);
        let mut decrypted = vec![0u8; original.len()];
        decrypt_state.xor_key_stream(&mut decrypted, &encrypted);
        assert_eq!(decrypted, original);
    }

    #[test]
    fn rc4_independent_states() {
        let key = b"16bytekeypadding";
        let mut s1 = RC4State::new(key);
        let mut s2 = RC4State::new(key);
        // Both start at same state
        let mut out1 = [0u8; 10];
        let mut out2 = [0u8; 10];
        s1.xor_key_stream(&mut out1, &[1u8; 10]);
        s2.xor_key_stream(&mut out2, &[1u8; 10]);
        assert_eq!(out1, out2);
    }
}
