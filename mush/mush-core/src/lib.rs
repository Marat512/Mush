pub mod crdt;
pub mod crypto;
pub mod framing;
pub mod identity;
pub mod sensors;
pub mod sns;
pub mod transport;

/// Standard base64 alphabet (RFC 4648) with padding. Implemented locally to
/// keep the dependency footprint matching MUSH-A'-CLI-SPEC.
pub mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
            out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6) as usize & 0x3f] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[n as usize & 0x3f] as char
            } else {
                '='
            });
        }
        out
    }

    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let bytes: Vec<u8> = s.bytes().filter(|b| !b" \t\r\n".contains(b)).collect();
        if !bytes.len().is_multiple_of(4) {
            return None;
        }
        let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
        for chunk in bytes.chunks(4) {
            let pad = chunk.iter().filter(|&&b| b == b'=').count();
            if pad > 2 || chunk[..4 - pad].iter().any(|&b| value_of(b).is_none()) {
                return None;
            }
            let n = (0..4)
                .map(|i| value_of(chunk[i]).unwrap_or(0) as u32)
                .fold(0u32, |acc, v| (acc << 6) | v);
            out.push((n >> 16) as u8);
            if pad < 2 {
                out.push((n >> 8) as u8);
            }
            if pad < 1 {
                out.push(n as u8);
            }
        }
        Some(out)
    }

    fn value_of(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn roundtrip() {
            let cases: Vec<Vec<u8>> = vec![
                vec![],
                vec![0x66],
                vec![0x66, 0x6f],
                vec![0x66, 0x6f, 0x6f],
                b"hello world, this is mush".to_vec(),
                (0u8..=255).collect(),
            ];
            for case in cases {
                assert_eq!(decode(&encode(&case)).unwrap(), case);
            }
        }

        #[test]
        fn known_vectors() {
            assert_eq!(encode(b""), "");
            assert_eq!(encode(b"f"), "Zg==");
            assert_eq!(encode(b"fo"), "Zm8=");
            assert_eq!(encode(b"foo"), "Zm9v");
        }
    }
}
