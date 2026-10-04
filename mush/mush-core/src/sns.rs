use std::collections::HashMap;
use std::time::{Duration, Instant};

use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::crypto;
use crate::identity::Identity;

pub const SNS_DOMAIN: &[u8] = b"SNS1";
pub const HKDF_INFO: &[u8] = b"Mush-SNS-v1";
pub const NONCE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Wire representation of one SNS handshake message (frame type 0x03).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeMessage {
    /// X25519 ephemeral public key (32 bytes).
    pub ephemeral: [u8; 32],
    /// 16-byte random nonce.
    pub nonce: [u8; 16],
    /// Ed25519 identity pubkey of the sender.
    pub identity_pubkey: [u8; 32],
    /// Ed25519 signature over ephemeral || nonce || (peer_nonce ||) || "SNS1" (64 bytes).
    pub signature: Vec<u8>,
    /// Display name (debug/UX aid, sent in clear — acceptable for A').
    pub name: String,
    /// Decoy EIDs this node currently advertises (honeypot publication, A' debug mode).
    pub decoys: Vec<[u8; 32]>,
}

impl HandshakeMessage {
    /// Signed payload for the initiator: ephemeral || nonce || "SNS1".
    fn initiator_payload(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(32 + 16 + SNS_DOMAIN.len());
        v.extend_from_slice(&self.ephemeral);
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(SNS_DOMAIN);
        v
    }

    /// Signed payload for the responder: ephemeral || nonce || nonce_A || "SNS1".
    fn responder_payload(&self, peer_nonce: &[u8; 16]) -> Vec<u8> {
        let mut v = Vec::with_capacity(32 + 16 + 16 + SNS_DOMAIN.len());
        v.extend_from_slice(&self.ephemeral);
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(peer_nonce);
        v.extend_from_slice(SNS_DOMAIN);
        v
    }
}

/// Established session: peer identity + symmetric session key.
#[derive(Debug, Clone)]
pub struct SnsSession {
    pub peer_pubkey: [u8; 32],
    pub peer_name: String,
    pub session_key: [u8; 32],
}

fn derive_session_key(
    my_ephemeral: &StaticSecret,
    peer_ephemeral: &[u8; 32],
    nonce_a: &[u8; 16],
    nonce_b: &[u8; 16],
) -> [u8; 32] {
    let peer_pub = PublicKey::from(*peer_ephemeral);
    let shared = my_ephemeral.diffie_hellman(&peer_pub);
    let mut salt = [0u8; 32];
    salt[..16].copy_from_slice(nonce_a);
    salt[16..].copy_from_slice(nonce_b);
    crypto::hkdf_derive(shared.as_bytes(), &salt, HKDF_INFO)
}

/// Side A of the handshake (the one that joined via invite-link / scanned QR).
pub struct HandshakeInitiator {
    identity: Identity,
    ephemeral: StaticSecret,
    nonce_a: [u8; 16],
}

impl HandshakeInitiator {
    pub fn new(identity: Identity, nonce_a: [u8; 16]) -> Self {
        let mut scalar = [0u8; 32];
        OsRng.fill_bytes(&mut scalar);
        HandshakeInitiator {
            identity,
            ephemeral: StaticSecret::from(scalar),
            nonce_a,
        }
    }

    pub fn nonce_a(&self) -> [u8; 16] {
        self.nonce_a
    }

    pub fn start(&self, name: &str, decoys: Vec<[u8; 32]>) -> HandshakeMessage {
        let msg = HandshakeMessage {
            ephemeral: PublicKey::from(&self.ephemeral).to_bytes(),
            nonce: self.nonce_a,
            identity_pubkey: self.identity.pubkey_bytes(),
            signature: Vec::new(),
            name: name.to_string(),
            decoys,
        };
        let mut msg = msg;
        msg.signature = self.identity.sign(&msg.initiator_payload()).to_vec();
        msg
    }

    /// Process the responder's reply. Verifies signature and nonce binding.
    pub fn finish(self, reply: &HandshakeMessage) -> Result<SnsSession, HandshakeError> {
        let Ok(signature) = <[u8; 64]>::try_from(reply.signature.as_slice()) else {
            return Err(HandshakeError::BadSignature);
        };
        if !Identity::verify(
            &reply.identity_pubkey,
            &reply.responder_payload(&self.nonce_a),
            &signature,
        ) {
            return Err(HandshakeError::BadSignature);
        }
        let session_key = derive_session_key(
            &self.ephemeral,
            &reply.ephemeral,
            &self.nonce_a,
            &reply.nonce,
        );
        Ok(SnsSession {
            peer_pubkey: reply.identity_pubkey,
            peer_name: reply.name.clone(),
            session_key,
        })
    }
}

/// Side B of the handshake (the one that showed the QR / printed the invite).
pub struct HandshakeResponder {
    identity: Identity,
    expected_nonce: Option<[u8; 16]>,
    seen_nonces: HashMap<[u8; 16], Instant>,
}

impl HandshakeResponder {
    /// `expected_nonce`: nonce from the invite-link; if set, the initiator's
    /// nonce must match it (replay of stale invites is rejected).
    pub fn new(identity: Identity, expected_nonce: Option<[u8; 16]>) -> Self {
        HandshakeResponder {
            identity,
            expected_nonce,
            seen_nonces: HashMap::new(),
        }
    }

    pub fn respond(
        &mut self,
        offer: &HandshakeMessage,
        name: &str,
        decoys: Vec<[u8; 32]>,
    ) -> Result<(HandshakeMessage, SnsSession), HandshakeError> {
        self.purge_expired();
        if self.seen_nonces.contains_key(&offer.nonce) {
            return Err(HandshakeError::Replay);
        }
        if let Some(expected) = self.expected_nonce {
            if expected != offer.nonce {
                return Err(HandshakeError::NonceMismatch);
            }
        }
        let Ok(offer_signature) = <[u8; 64]>::try_from(offer.signature.as_slice()) else {
            return Err(HandshakeError::BadSignature);
        };
        if !Identity::verify(
            &offer.identity_pubkey,
            &offer.initiator_payload(),
            &offer_signature,
        ) {
            return Err(HandshakeError::BadSignature);
        }
        self.seen_nonces.insert(offer.nonce, Instant::now());

        let mut scalar = [0u8; 32];
        OsRng.fill_bytes(&mut scalar);
        let ephemeral = StaticSecret::from(scalar);
        let nonce_b = rand_nonce();
        let mut reply = HandshakeMessage {
            ephemeral: PublicKey::from(&ephemeral).to_bytes(),
            nonce: nonce_b,
            identity_pubkey: self.identity.pubkey_bytes(),
            signature: Vec::new(),
            name: name.to_string(),
            decoys,
        };
        reply.signature = self
            .identity
            .sign(&reply.responder_payload(&offer.nonce))
            .to_vec();

        let session_key = derive_session_key(&ephemeral, &offer.ephemeral, &offer.nonce, &nonce_b);
        let session = SnsSession {
            peer_pubkey: offer.identity_pubkey,
            peer_name: offer.name.clone(),
            session_key,
        };
        Ok((reply, session))
    }

    fn purge_expired(&mut self) {
        let cutoff = Instant::now()
            .checked_sub(NONCE_TTL)
            .unwrap_or_else(Instant::now);
        self.seen_nonces.retain(|_, at| *at > cutoff);
    }
}

fn rand_nonce() -> [u8; 16] {
    let mut n = [0u8; 16];
    OsRng.fill_bytes(&mut n);
    n
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    BadSignature,
    NonceMismatch,
    Replay,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::BadSignature => write!(f, "invalid signature"),
            HandshakeError::NonceMismatch => write!(f, "nonce does not match invite"),
            HandshakeError::Replay => write!(f, "nonce already seen (replay)"),
        }
    }
}

impl std::error::Error for HandshakeError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full handshake over real identities. Returns (initiator session, responder session).
    fn run_handshake() -> (SnsSession, SnsSession, [u8; 32], [u8; 32]) {
        let id_a = Identity::generate();
        let id_b = Identity::generate();
        let pk_a = id_a.pubkey_bytes();
        let pk_b = id_b.pubkey_bytes();
        let nonce_a = rand_nonce();
        let init = HandshakeInitiator::new(id_a, nonce_a);
        let offer = init.start("alice", vec![[7u8; 32]]);
        let mut resp = HandshakeResponder::new(id_b, Some(nonce_a));
        let (reply, session_b) = resp.respond(&offer, "bob", vec![]).expect("respond");
        let session_a = init.finish(&reply).expect("finish");
        (session_a, session_b, pk_a, pk_b)
    }

    #[test]
    fn full_handshake_derives_equal_session_keys() {
        let (session_a, session_b, pk_a, pk_b) = run_handshake();
        assert_eq!(session_a.session_key, session_b.session_key);
        assert_eq!(session_a.peer_pubkey, pk_b);
        assert_eq!(session_b.peer_pubkey, pk_a);
        assert_eq!(session_a.peer_name, "bob");
        assert_eq!(session_b.peer_name, "alice");
        assert_ne!(session_a.session_key, [0u8; 32]);
    }

    #[test]
    fn wrong_invite_nonce_rejected() {
        let id_a = Identity::generate();
        let id_b = Identity::generate();
        let init = HandshakeInitiator::new(id_a, rand_nonce());
        let offer = init.start("alice", vec![]);
        let mut resp = HandshakeResponder::new(id_b, Some(rand_nonce())); // different nonce
        assert_eq!(
            resp.respond(&offer, "bob", vec![]).unwrap_err(),
            HandshakeError::NonceMismatch
        );
    }

    #[test]
    fn bad_signature_rejected() {
        let id_a = Identity::generate();
        let id_b = Identity::generate();
        let init = HandshakeInitiator::new(id_a, rand_nonce());
        let mut offer = init.start("alice", vec![]);
        offer.signature[0] ^= 1;
        let mut resp = HandshakeResponder::new(id_b, None);
        assert_eq!(
            resp.respond(&offer, "bob", vec![]).unwrap_err(),
            HandshakeError::BadSignature
        );
    }

    #[test]
    fn replayed_nonce_rejected() {
        let id_a = Identity::generate();
        let id_b = Identity::generate();
        let init = HandshakeInitiator::new(id_a, rand_nonce());
        let offer = init.start("alice", vec![]);
        let mut resp = HandshakeResponder::new(id_b, None);
        resp.respond(&offer, "bob", vec![]).expect("first ok");
        assert_eq!(
            resp.respond(&offer, "bob", vec![]).unwrap_err(),
            HandshakeError::Replay
        );
    }
}
