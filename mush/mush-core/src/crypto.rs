use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

pub mod identicon;
pub mod ratchet;
pub mod safety;
pub mod vectors;

/// Derive a 32-byte symmetric key via HKDF-SHA256
/// (`salt` and `info` exactly as specified by the protocol).
pub fn hkdf_derive(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = [0u8; 32];
    hkdf.expand(info, &mut okm)
        .expect("32 bytes is within HKDF-SHA256 limits");
    okm
}

/// AES-256-GCM encrypt. `nonce` must be 12 bytes. Ciphertext includes the 16-byte tag.
pub fn aes_gcm_encrypt(key: &[u8; 32], plaintext: &[u8], nonce: &[u8; 12]) -> Vec<u8> {
    aes_gcm_encrypt_aad(key, plaintext, nonce, b"")
}

/// AES-256-GCM encrypt with associated data (этап A: X3DH first message, ratchet).
pub fn aes_gcm_encrypt_aad(
    key: &[u8; 32],
    plaintext: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("aes-gcm encrypt")
}

/// AES-256-GCM decrypt. Returns None on wrong key / corrupted ciphertext.
pub fn aes_gcm_decrypt(key: &[u8; 32], ciphertext: &[u8], nonce: &[u8; 12]) -> Option<Vec<u8>> {
    aes_gcm_decrypt_aad(key, ciphertext, nonce, b"")
}

/// AES-256-GCM decrypt with associated data. None on wrong key/AAD/ciphertext.
pub fn aes_gcm_decrypt_aad(
    key: &[u8; 32],
    ciphertext: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
) -> Option<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    #[test]
    fn hkdf_matches_expected_output() {
        // RFC 5869 test case 1 (SHA-256), truncated to 32 bytes of OKM.
        let ikm = [0x0b; 22];
        let salt = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        ];
        let info = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
        let okm = hkdf_derive(&ikm, &salt, &info);
        let expected_prefix = [
            0x3c, 0xb2, 0x5f, 0x25, 0xfa, 0xac, 0xd5, 0x7a, 0x90, 0x43, 0x4f, 0x64, 0xd0, 0x36,
            0x2f, 0x2a, 0x2d, 0x2d, 0x0a, 0x90, 0xcf, 0x1a, 0x5a, 0x4c, 0x5d, 0xb0, 0x2d, 0x56,
            0xec, 0xc4, 0xc5, 0xbf, 0x34, 0x00, 0x72, 0x08, 0xd5, 0xb8, 0x87, 0x18, 0x58, 0x65,
        ];
        assert_eq!(okm[..], expected_prefix[..32]);
    }

    #[test]
    fn aes_gcm_roundtrip() {
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut key);
        OsRng.fill_bytes(&mut nonce);
        let plaintext = b"mush message";
        let ct = aes_gcm_encrypt(&key, plaintext, &nonce);
        assert_ne!(ct, plaintext);
        assert_eq!(aes_gcm_decrypt(&key, &ct, &nonce).unwrap(), plaintext);
    }

    #[test]
    fn aes_gcm_rejects_wrong_key_and_nonce() {
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut key);
        OsRng.fill_bytes(&mut nonce);
        let ct = aes_gcm_encrypt(&key, b"data", &nonce);
        let mut wrong_key = key;
        wrong_key[0] ^= 1;
        assert!(aes_gcm_decrypt(&wrong_key, &ct, &nonce).is_none());
        let mut wrong_nonce = nonce;
        wrong_nonce[0] ^= 1;
        assert!(aes_gcm_decrypt(&key, &ct, &wrong_nonce).is_none());
    }

    use rand::rngs::OsRng;
}
