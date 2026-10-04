//! Этап A: X3DH + Double Ratchet (спека этапа A, X3DH по Signal-спецификации).
//!
//! Модуль живёт РЯДОМ с текущим симметричным `crypto::session` (sns.rs):
//! переключение протокола — runtime-конфигом на уровне Node, так что CLI-smoke
//! и Android остаются рабочими на старом протоколе, пока ratchet не интегрирован.
//!
//! Сегодняшний слой — X3DH: обмен бандлами pre-keys, выработка общего
//! `SessionStart { sk, ad }` и AEAD-защита первого сообщения с AD = IK_A||IK_B.
//! Double Ratchet (root/chain keys, skipped keys) — следующий слой поверх
//! `SessionStart`.

use std::collections::HashMap;

use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::crypto::{aes_gcm_decrypt_aad, aes_gcm_encrypt_aad, hkdf_derive};
use crate::identity::Identity;

/// Паддинг DH4, когда one-time pre-key отсутствует (как в X3DH-спецификации).
const F_PADDING: [u8; 32] = [0x57; 32];

fn x25519_secret() -> StaticSecret {
    let mut scalar = [0u8; 32];
    OsRng.fill_bytes(&mut scalar);
    StaticSecret::from(scalar)
}

fn x25519_public(secret: &StaticSecret) -> [u8; 32] {
    PublicKey::from(secret).to_bytes()
}

fn dh_x_x(secret: &StaticSecret, peer_public: &[u8; 32]) -> [u8; 32] {
    let peer = PublicKey::from(*peer_public);
    *secret.diffie_hellman(&peer).as_bytes()
}

/// DH между нашим X25519-секретом и Ed25519-публичным ключом пира
/// (identity-ключи конвертируются в Montgomery-форму).
fn dh_x_ed(secret: &StaticSecret, peer_ed25519: &[u8; 32]) -> Option<[u8; 32]> {
    let mont = Identity::ed_pubkey_to_x25519(peer_ed25519)?;
    Some(dh_x_x(secret, &mont))
}

/// Подписанный pre-key: среднесрочный X25519 ключ, заверен identity-ключом.
pub struct SignedPreKey {
    pub key_id: u32,
    secret: StaticSecret,
    pub public: [u8; 32],
    pub signature: [u8; 64],
}

impl SignedPreKey {
    pub fn generate(identity: &Identity, key_id: u32) -> Self {
        let secret = x25519_secret();
        let public = x25519_public(&secret);
        let mut sig_msg = Vec::with_capacity(40 + 32);
        sig_msg.extend_from_slice(b"mush-spk");
        sig_msg.extend_from_slice(&key_id.to_le_bytes());
        sig_msg.extend_from_slice(&public);
        SignedPreKey {
            key_id,
            secret,
            public,
            signature: identity.sign(&sig_msg),
        }
    }

    /// Проверка подписи SPK чужим identity-ключом (initiator обязан проверять).
    pub fn verify_signature(
        identity_pubkey: &[u8; 32],
        key_id: u32,
        public: &[u8; 32],
        sig: &[u8; 64],
    ) -> bool {
        let mut sig_msg = Vec::with_capacity(40 + 32);
        sig_msg.extend_from_slice(b"mush-spk");
        sig_msg.extend_from_slice(&key_id.to_le_bytes());
        sig_msg.extend_from_slice(public);
        Identity::verify(identity_pubkey, &sig_msg, sig)
    }
}

/// Одноразовый pre-key. Удаляется из хранилища сразу после использования.
pub struct OneTimePreKey {
    pub key_id: u32,
    secret: StaticSecret,
    pub public: [u8; 32],
}

/// Бандл, публикуемый в сеть (в A'-CLI — TCP-broadcast; позже — mesh).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreKeyBundle {
    /// Ed25519 identity key (EID).
    pub identity: [u8; 32],
    pub spk_id: u32,
    /// X25519 signed pre-key.
    pub spk: [u8; 32],
    /// Подпись SPK identity-ключом, скаляры R и S (serde не умеет [u8;64]).
    pub spk_sig_r: [u8; 32],
    pub spk_sig_s: [u8; 32],
    /// Одноразовый pre-key; None, если очередь исчерпана (X3DH без DH4).
    pub opk_id: Option<u32>,
    pub opk: Option<[u8; 32]>,
}

/// Хранилище pre-keys стороны, принимающей соединения (Bob).
/// Узел генерирует N одноразовых ключей и пополняет очередь по мере траты.
pub struct PreKeyStore {
    identity: Identity,
    spk: SignedPreKey,
    /// Неопубликованные ключи; при публикации бандла один уходит в published.
    pending: HashMap<u32, OneTimePreKey>,
    /// Опубликованные, ещё не использованные. Один ожидающий handshake.
    published: HashMap<u32, OneTimePreKey>,
    next_opk_id: u32,
}

impl PreKeyStore {
    pub fn new(identity: Identity) -> Self {
        PreKeyStore {
            spk: SignedPreKey::generate(&identity, 1),
            identity,
            pending: HashMap::new(),
            published: HashMap::new(),
            next_opk_id: 1,
        }
    }

    /// Сгенерировать `n` одноразовых ключей в очередь.
    pub fn replenish(&mut self, n: usize) {
        for _ in 0..n {
            let secret = x25519_secret();
            let public = x25519_public(&secret);
            let id = self.next_opk_id;
            self.next_opk_id += 1;
            self.pending.insert(
                id,
                OneTimePreKey {
                    key_id: id,
                    secret,
                    public,
                },
            );
        }
    }

    /// Публикация бандла: забирает один pending-ключ в published.
    /// Если очередь пуста — бандл без OPK (протокол допускает).
    pub fn publish_bundle(&mut self) -> PreKeyBundle {
        let (opk_id, opk) = match self.pending.keys().min().copied() {
            Some(id) => {
                let key = self.pending.remove(&id).expect("pending key");
                let public = key.public;
                self.published.insert(id, key);
                (Some(id), Some(public))
            }
            None => (None, None),
        };
        PreKeyBundle {
            identity: self.identity.pubkey_bytes(),
            spk_id: self.spk.key_id,
            spk: self.spk.public,
            spk_sig_r: self.spk.signature[..32].try_into().expect("sig R half"),
            spk_sig_s: self.spk.signature[32..].try_into().expect("sig S half"),
            opk_id,
            opk,
        }
    }

    /// Сторона responder: завершить X3DH по `X3dhInit` initiator'а.
    /// Возвращает `SessionStart` и открытый первый текст; OPK при использовании
    /// сжигается. Повторный вызов с тем же init — None (one-time семантика).
    pub fn complete_handshake(&mut self, init: &X3dhInit) -> Option<(SessionStart, Vec<u8>)> {
        if init.used_opk_id.is_some() && !self.published.contains_key(init.used_opk_id.as_ref()?) {
            // OPK не наш или уже сожжён — возможная попытка повтора.
            return None;
        }
        let my_identity = self.identity.pubkey_bytes();
        let ad = associated_data(&init.identity, &my_identity);

        // DH1 = DH(SPK_B, IK_A); DH2 = DH(IK_B, EK_A); DH3 = DH(SPK_B, EK_A).
        let dh1 = dh_x_ed(&self.spk.secret, &init.identity)?;
        let id_secret = self.identity.to_x25519_secret();
        let dh2 = dh_x_x(&id_secret, &init.eph);
        let dh3 = dh_x_x(&self.spk.secret, &init.eph);
        let dh4 = match init.used_opk_id {
            Some(opk_id) => {
                let opk = self.published.remove(&opk_id)?;
                dh_x_x(&opk.secret, &init.eph)
            }
            None => F_PADDING,
        };
        let sk = derive_sk(&ad, &[dh1, dh2, dh3, dh4]);
        let first = aes_gcm_decrypt_aad(&sk, &init.ciphertext, &init.nonce, &ad)?;
        Some((SessionStart { sk, ad }, first))
    }

    pub fn identity_pubkey(&self) -> [u8; 32] {
        self.identity.pubkey_bytes()
    }

    pub fn signed_pre_key_id(&self) -> u32 {
        self.spk.key_id
    }
}

/// Первый фрейм сессии: initiator → responder. Передаётся открыто (ephemeral
/// не подписывается; аутентичность обеспечивается AEAD первого сообщения с AD).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X3dhInit {
    /// IK_A (Ed25519).
    pub identity: [u8; 32],
    /// EK_A (X25519 ephemeral).
    pub eph: [u8; 32],
    pub bundle_spk_id: u32,
    pub used_opk_id: Option<u32>,
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 12],
}

/// Результат X3DH: общий секрет и associated data обеих сторон.
/// Поверх этого строится Double Ratchet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStart {
    pub sk: [u8; 32],
    pub ad: [u8; 64],
}

pub fn associated_data(ik_a: &[u8; 32], ik_b: &[u8; 32]) -> [u8; 64] {
    let mut ad = [0u8; 64];
    ad[..32].copy_from_slice(ik_a);
    ad[32..].copy_from_slice(ik_b);
    ad
}

fn sig64(r: &[u8; 32], s: &[u8; 32]) -> [u8; 64] {
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(r);
    sig[32..].copy_from_slice(s);
    sig
}

fn derive_sk(ad: &[u8; 64], dhs: &[[u8; 32]; 4]) -> [u8; 32] {
    let mut ikm = Vec::with_capacity(128);
    for dh in dhs {
        ikm.extend_from_slice(dh);
    }
    hkdf_derive(&ikm, b"mush-x3dh-sk", ad)
}

/// Сторона initiator (Alice): по бандлу Bob и identity Alice стартует сессию.
pub struct X3dhInitiator;

impl X3dhInitiator {
    /// Проверяет подпись SPK, считает SK, шифрует `first_message` с AD.
    /// Возвращает SessionStart (идентичный у Bob) и открытый X3dhInit.
    pub fn start(
        identity: &Identity,
        bundle: &PreKeyBundle,
        first_message: &[u8],
    ) -> Option<(SessionStart, X3dhInit)> {
        if !SignedPreKey::verify_signature(
            &bundle.identity,
            bundle.spk_id,
            &bundle.spk,
            &sig64(&bundle.spk_sig_r, &bundle.spk_sig_s),
        ) {
            return None;
        }
        let eph_secret = x25519_secret();
        let eph_public = x25519_public(&eph_secret);
        let my_identity = identity.pubkey_bytes();
        let ad = associated_data(&my_identity, &bundle.identity);

        // DH1 = DH(IK_A, SPK_B); DH2 = DH(EK_A, IK_B); DH3 = DH(EK_A, SPK_B).
        let dh1 = dh_x_x(&identity.to_x25519_secret(), &bundle.spk);
        let dh2 = dh_x_x(
            &eph_secret,
            &Identity::ed_pubkey_to_x25519(&bundle.identity)?,
        );
        let dh3 = dh_x_x(&eph_secret, &bundle.spk);
        let dh4 = match (&bundle.opk_id, &bundle.opk) {
            (Some(_), Some(opk)) => dh_x_x(&eph_secret, opk),
            // Бандл без OPK — DH4 = padding (по X3DH).
            (None, None) => F_PADDING,
            // Рассинхрон id/ключа — битый бандл.
            _ => return None,
        };
        let sk = derive_sk(&ad, &[dh1, dh2, dh3, dh4]);
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = aes_gcm_encrypt_aad(&sk, first_message, &nonce, &ad);
        (
            SessionStart { sk, ad },
            X3dhInit {
                identity: my_identity,
                eph: eph_public,
                bundle_spk_id: bundle.spk_id,
                used_opk_id: bundle.opk_id,
                ciphertext,
                nonce,
            },
        )
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (PreKeyStore, Identity) {
        let bob_id = Identity::generate();
        let mut bob = PreKeyStore::new(bob_id.clone());
        bob.replenish(3);
        (bob, bob_id)
    }

    #[test]
    fn x3dh_full_handshake_agrees_on_sk_and_ad() {
        let (mut bob, _bob_id) = setup();
        let alice = Identity::generate();
        let bundle = bob.publish_bundle();

        let (alice_start, init) =
            X3dhInitiator::start(&alice, &bundle, b"hello bob").expect("initiator");
        let (bob_start, first) = bob.complete_handshake(&init).expect("responder");

        assert_eq!(alice_start, bob_start);
        assert_eq!(first, b"hello bob");
    }

    #[test]
    fn x3dh_first_message_aad_binds_identities() {
        let (mut bob, _bob_id) = setup();
        let alice = Identity::generate();
        let bundle = bob.publish_bundle();
        let (_start, mut init) = X3dhInitiator::start(&alice, &bundle, b"hi").expect("initiator");
        // Подмена IK_A в init ломает AEAD (AD = IK_A || IK_B).
        init.identity = Identity::generate().pubkey_bytes();
        assert!(bob.complete_handshake(&init).is_none());
    }

    #[test]
    fn x3dh_rejects_tampered_spk_signature() {
        let (mut bob, _bob_id) = setup();
        let alice = Identity::generate();
        let mut bundle = bob.publish_bundle();
        bundle.spk[0] ^= 1; // подмена ключа, подпись больше не валидна
        assert!(X3dhInitiator::start(&alice, &bundle, b"hi").is_none());
    }

    #[test]
    fn x3dh_opk_is_one_time() {
        let (mut bob, _bob_id) = setup();
        let alice = Identity::generate();
        let bundle = bob.publish_bundle();
        assert!(bundle.opk_id.is_some(), "bundle должен содержать OPK");

        let (_start, init) = X3dhInitiator::start(&alice, &bundle, b"first").expect("initiator");
        // Первый handshake проходит, OPK сожжён.
        assert!(bob.complete_handshake(&init).is_some());
        // Повтор с тем же init: OPK уже не в published — отказ (anti-replay).
        assert!(bob.complete_handshake(&init).is_none());
    }

    #[test]
    fn x3dh_works_without_opk() {
        let bob_id = Identity::generate();
        let mut bob = PreKeyStore::new(bob_id); // replenish не вызывали
        let alice = Identity::generate();
        let bundle = bob.publish_bundle();
        assert!(bundle.opk.is_none());

        let (alice_start, init) =
            X3dhInitiator::start(&alice, &bundle, b"no opk").expect("initiator");
        let (bob_start, first) = bob.complete_handshake(&init).expect("responder");
        assert_eq!(alice_start, bob_start);
        assert_eq!(first, b"no opk");
    }

    #[test]
    fn x3dh_distinct_handshakes_produce_distinct_sks() {
        let (mut bob, _bob_id) = setup();
        let a1 = Identity::generate();
        let a2 = Identity::generate();
        let b1 = bob.publish_bundle();
        let (s1, i1) = X3dhInitiator::start(&a1, &b1, b"m1").expect("h1");
        let b2 = bob.publish_bundle();
        let (s2, i2) = X3dhInitiator::start(&a2, &b2, b"m2").expect("h2");
        assert_ne!(s1.sk, s2.sk);
        assert!(bob.complete_handshake(&i1).is_some());
        assert!(bob.complete_handshake(&i2).is_some());
    }

    #[test]
    fn replenish_refills_pending_queue() {
        let (mut bob, _bob_id) = setup();
        bob.replenish(2);
        for _ in 0..5 {
            let _bundle = bob.publish_bundle();
        }
        // После 5 публикаций очередь пуста (3+2 ключей израсходованы) —
        // бандл без OPK.
        let empty_bundle = bob.publish_bundle();
        assert!(empty_bundle.opk.is_none());
        bob.replenish(1);
        assert!(bob.publish_bundle().opk.is_some());
    }
}
