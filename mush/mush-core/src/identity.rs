use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use sha2::Digest;
use x25519_dalek::StaticSecret;

/// Ed25519 identity keypair of a node. Generated once at first start.
#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
}

impl Identity {
    pub fn generate() -> Self {
        Identity {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// 32-byte Ed25519 public key — the node's identity (EID).
    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Sign `msg` with the identity key. Returns a 64-byte Ed25519 signature.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let sig: Signature = self.signing_key.sign(msg);
        sig.to_bytes()
    }

    /// Verify `sig` (64 bytes) of `msg` against `pubkey` (32 bytes).
    pub fn verify(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(verifying) = VerifyingKey::from_bytes(pubkey) else {
            return false;
        };
        let signature = Signature::from_bytes(sig);
        verifying.verify(msg, &signature).is_ok()
    }

    /// Convert the Ed25519 secret key to an X25519 secret key
    /// (libsodium-compatible: SHA-512 of the seed, first 32 bytes, clamped).
    pub fn to_x25519_secret(&self) -> StaticSecret {
        let hash = sha2::Sha512::digest(self.signing_key.to_bytes());
        let mut scalar: [u8; 32] = hash[..32].try_into().expect("sha512 is 64 bytes");
        clamp_scalar(&mut scalar);
        StaticSecret::from(scalar)
    }

    /// Convert an Ed25519 public key to its X25519 Montgomery form.
    /// Returns None if the point does not decompress.
    pub fn ed_pubkey_to_x25519(pubkey: &[u8; 32]) -> Option<[u8; 32]> {
        let compressed = curve25519_dalek::edwards::CompressedEdwardsY(*pubkey);
        compressed.decompress().map(|point| point.to_montgomery().to_bytes())
    }

    /// Shortcut: X25519 shared secret between this identity and a peer's
    /// Ed25519 public key (identity DH — used by tests and future stages).
    pub fn dh_with(&self, peer_ed25519_pubkey: &[u8; 32]) -> Option<[u8; 32]> {
        let peer_mont = Self::ed_pubkey_to_x25519(peer_ed25519_pubkey)?;
        let peer_pub = x25519_dalek::PublicKey::from(peer_mont);
        let secret = self.to_x25519_secret();
        let shared = secret.diffie_hellman(&peer_pub);
        Some(*shared.as_bytes())
    }
}

fn clamp_scalar(scalar: &mut [u8; 32]) {
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use x25519_dalek::PublicKey;

    #[test]
    fn sign_verify_roundtrip() {
        let id = Identity::generate();
        let msg = b"hello mush";
        let sig = id.sign(msg);
        assert!(Identity::verify(&id.pubkey_bytes(), msg, &sig));
    }

    #[test]
    fn verify_rejects_wrong_key_and_tampered_msg() {
        let a = Identity::generate();
        let b = Identity::generate();
        let sig = a.sign(b"payload");
        assert!(!Identity::verify(&b.pubkey_bytes(), b"payload", &sig));
        assert!(!Identity::verify(&a.pubkey_bytes(), b"payloaX", &sig));
    }

    #[test]
    fn ed25519_to_x25519_dh_agreement() {
        // The most error-prone conversion (ТЗ: проверить обязательно):
        // DH(A_priv, B_pub) == DH(B_priv, A_pub) after Ed25519 -> X25519 conversion.
        let a = Identity::generate();
        let b = Identity::generate();
        let shared_a = a.dh_with(&b.pubkey_bytes()).expect("dh a");
        let shared_b = b.dh_with(&a.pubkey_bytes()).expect("dh b");
        assert_eq!(shared_a, shared_b);
    }

    #[test]
    fn x25519_conversion_produces_valid_points() {
        for _ in 0..20 {
            let id = Identity::generate();
            let mont = Identity::ed_pubkey_to_x25519(&id.pubkey_bytes()).expect("montgomery");
            let secret = id.to_x25519_secret();
            let pub_from_secret = PublicKey::from(&secret);
            // The converted secret's public must equal the converted public key.
            assert_eq!(pub_from_secret.to_bytes(), mont);
        }
    }
}
