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
    /// Tombstone сожжённых OPK: id -> used_at. TTL USED_PREKEY_TTL_MS.
    /// Ids монотонны (переиспользования нет) — множество чисто
    /// детекционное: повторное использование = атака/ошибка (MUSH-A-SPEC §1).
    used_pre_keys: std::collections::BTreeMap<u32, u64>,
    next_opk_id: u32,
}

impl PreKeyStore {
    pub fn new(identity: Identity) -> Self {
        PreKeyStore {
            spk: SignedPreKey::generate(&identity, 1),
            identity,
            pending: HashMap::new(),
            published: HashMap::new(),
            used_pre_keys: std::collections::BTreeMap::new(),
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
                // Tombstone: повторное использование — атака/ошибка.
                if self.used_pre_keys.contains_key(&opk_id) {
                    return None;
                }
                let opk = self.published.remove(&opk_id)?;
                self.used_pre_keys.insert(opk_id, now_millis());
                self.prune_used_pre_keys();
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

    /// Секрет SPK для построения responder-стороны ratchet-сессии
    /// (DH(SPK, DHS_A) на первом сообщении).
    pub fn signed_pre_key_secret(&self) -> &StaticSecret {
        &self.spk.secret
    }

    /// TTL-prune tombstone used_pre_keys.
    fn prune_used_pre_keys(&mut self) {
        let cutoff = now_millis().saturating_sub(USED_PREKEY_TTL_MS);
        self.used_pre_keys.retain(|_, used_at| *used_at > cutoff);
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

// ---------------------------------------------------------------------------
// Double Ratchet поверх SessionStart (Signal DR).
// ---------------------------------------------------------------------------

/// Заголовок ratchet-сообщения (открытая часть).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatchetHeader {
    /// Текущий DH-ratchet публичный ключ отправителя.
    pub ratchet_key: [u8; 32],
    /// Индекс msg key этого сообщения в его цепочке (n).
    pub chain_index: u32,
    /// Длина предыдущей sending-цепочки отправителя (pn) — для skipped keys.
    pub prev_chain_len: u32,
}

/// Шифрованное ratchet-сообщение (wire-форма).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatchetMessage {
    pub header: RatchetHeader,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

/// Chain key: секрет цепочки + сколько msg keys уже извлечено.
#[derive(Debug, Clone, Copy)]
struct ChainKey {
    key: [u8; 32],
    index: u32,
}

impl ChainKey {
    /// Единственный шаг цепочки: следующий chain key + msg key для текущего index.
    fn step(&self) -> (ChainKey, [u8; 32]) {
        let next = ChainKey {
            key: hkdf_derive(&self.key, b"mush-dr-ck", b""),
            index: self.index + 1,
        };
        let mk = hkdf_derive(&self.key, b"mush-dr-mk", b"");
        (next, mk)
    }
}

/// KDF корневой цепи на DH-шаге: новый root key + новый chain key.
fn root_kdf(root_key: &[u8; 32], dh_out: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let next_root = hkdf_derive(dh_out, root_key, b"mush-dr-root");
    let chain = hkdf_derive(dh_out, root_key, b"mush-dr-chain");
    (next_root, chain)
}

/// Потолок skipped keys (защита от DoS переполнением).
const MAX_SKIPPED: usize = 200;
/// Максимальный разрыв индексов, который мы готовы догнать внутри цепи.
const MAX_CHAIN_GAP: u32 = 100;
/// TTL skipped keys: 7 суток (MUSH-A-SPEC §3).
const SKIPPED_TTL_MS: u64 = 7 * 24 * 3600 * 1000;
/// LRU-потолок seen_ratchet_keys (MUSH-A-SPEC §4).
const SEEN_CAP: usize = 1000;
/// TTL записей seen_ratchet_keys: 30 суток.
const SEEN_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
/// TTL tombstone used_pre_keys: 30 суток (MUSH-A-SPEC §1).
const USED_PREKEY_TTL_MS: u64 = 30 * 24 * 3600 * 1000;

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Двусторонняя ratchet-сессия. Инициатор отправляет первым (после X3DH
/// first message); респондер не может шифровать, пока не получит хотя бы
/// одно сообщение (нет remote ratchet key).
pub struct RatchetSession {
    root_key: [u8; 32],
    /// Наш текущий DH-ratchet секрет. Заменяется на свежий при каждом
    /// получении НОВОГО ratchet-ключа пира.
    dhs: StaticSecret,
    dhr: Option<[u8; 32]>,
    send_chain: Option<ChainKey>,
    recv_chain: Option<ChainKey>,
    /// Длина предыдущей нашей sending-цепочки (для pn в заголовке).
    prev_send_len: u32,
    /// Пропущенные msg keys: (ratchet_key пира, индекс) -> (msg_key, created_at).
    /// TTL — SKIPPED_TTL_MS, prune при export и lazily при decrypt (MUSH-A-SPEC §3).
    skipped: std::collections::BTreeMap<([u8; 32], u32), ([u8; 32], u64)>,
    ad: [u8; 64],
    /// Responder-only: собственный SPK-секрет до обработки первого
    /// сообщения (recv-цепочка строится из DH(SPK, DHS_A)).
    spk_secret: Option<StaticSecret>,
    /// Все ratchet-ключи пира, через которые мы уже прошли (текущий +
    /// прежние). Ключ вне этого набора — НОВЫЙ (ratchet-шаг); ключ из
    /// набора, но не текущий — старая цепочка, только skipped-lookup.
    /// LRU cap SEEN_CAP, TTL SEEN_TTL_MS.
    seen_ratchet_keys: std::collections::HashMap<[u8; 32], u64>,
}

impl RatchetSession {
    /// Initiator (Alice). Начальная send-цепочка строится из
    /// DH(DHS_A, SPK_B) — тот же DH считает responder на первом сообщении,
    /// получая recv-цепочку. `spk_public` — signed pre-key пира из бандла.
    pub fn new_initiator(start: &SessionStart, spk_public: &[u8; 32]) -> Self {
        let dhs = x25519_secret();
        let dh0 = dh_x_x(&dhs, spk_public);
        let (root, send_ck) = root_kdf(&start.sk, &dh0);
        RatchetSession {
            root_key: root,
            dhs,
            dhr: Some(*spk_public),
            send_chain: Some(ChainKey {
                key: send_ck,
                index: 0,
            }),
            recv_chain: None,
            prev_send_len: 0,
            skipped: std::collections::BTreeMap::new(),
            ad: start.ad,
            spk_secret: None,
            seen_ratchet_keys: std::collections::HashMap::new(),
        }
    }

    /// Responder (Bob). До первого сообщения цепочек нет: recv-цепочка
    /// появится из DH(SPK_B, DHS_A) (ключ DHS_A приезжает в первом
    /// заголовке), send — из DH(DHS_B, DHS_A). `spk_secret` — собственный
    /// signed pre-key, которым шифровался бандл.
    pub fn new_responder(start: &SessionStart, spk_secret: StaticSecret) -> Self {
        RatchetSession {
            root_key: start.sk,
            dhs: x25519_secret(),
            dhr: None,
            send_chain: None,
            recv_chain: None,
            prev_send_len: 0,
            skipped: std::collections::BTreeMap::new(),
            ad: start.ad,
            spk_secret: Some(spk_secret),
            seen_ratchet_keys: std::collections::HashMap::new(),
        }
    }

    /// Шифрует сообщение. Респондер до обработки первого сообщения вернёт None.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Option<RatchetMessage> {
        let chain = self.send_chain.as_mut()?;
        let ratchet_key = x25519_public(&self.dhs);
        let (next, mk) = chain.step();
        let used_index = chain.index;
        *chain = next;
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = aes_gcm_encrypt_aad(&mk, plaintext, &nonce, &self.ad);
        Some(RatchetMessage {
            header: RatchetHeader {
                ratchet_key,
                chain_index: used_index,
                prev_chain_len: self.prev_send_len,
            },
            nonce,
            ciphertext,
        })
    }

    /// Дешифрует сообщение с поддержкой out-of-order (skipped keys).
    /// None при невалидном ключе, израсходованном индексе (replay) или
    /// слишком большом разрыве (DoS-страж).
    pub fn decrypt(&mut self, msg: &RatchetMessage) -> Option<Vec<u8>> {
        let remote_key = msg.header.ratchet_key;
        let n = msg.header.chain_index;

        // Первое сообщение responder'а: инициализация из SPK.
        if let Some(spk) = self.spk_secret.take() {
            let dh_recv = dh_x_x(&spk, &remote_key);
            let (root, ck) = root_kdf(&self.root_key, &dh_recv);
            self.root_key = root;
            self.recv_chain = Some(ChainKey { key: ck, index: 0 });
            // Send-цепочка: DH(текущий dhs, ключ первого сообщения) — парирует
            // recv-цепочку initiator'а на его следующем шаге.
            let dh_send = dh_x_x(&self.dhs, &remote_key);
            let (root, ck) = root_kdf(&self.root_key, &dh_send);
            self.root_key = root;
            self.send_chain = Some(ChainKey { key: ck, index: 0 });
            self.dhr = Some(remote_key);
            self.seen_ratchet_keys.insert(remote_key, now_millis());
            self.prev_send_len = 0;
        } else if self.dhr == Some(remote_key) {
            // Текущая цепочка — ничего не делаем.
            self.seen_ratchet_keys.insert(remote_key, now_millis());
        } else if let Some(ts) = self.seen_ratchet_keys.get_mut(&remote_key) {
            // Старая цепочка: сообщение открывается ТОЛЬКО skipped-ключом.
            *ts = now_millis();
            let (mk, _) = self.skipped.remove(&(remote_key, n))?;
            return aes_gcm_decrypt_aad(&mk, &msg.ciphertext, &msg.nonce, &self.ad);
        } else {
            // Новый ratchet-ключ пира: DH-ratchet шаг.
            self.seen_ratchet_keys.insert(remote_key, now_millis());
            self.evict_seen_if_needed();
            self.ratchet(remote_key, msg.header.prev_chain_len)?;
        }

        let chain = self.recv_chain.as_mut()?;

        if n < chain.index {
            // Позднее сообщение: msg key из skipped, извлекается (одноразово).
            let (mk, _) = self.skipped.remove(&(remote_key, n))?;
            return aes_gcm_decrypt_aad(&mk, &msg.ciphertext, &msg.nonce, &self.ad);
        }

        let gap = n - chain.index;
        if gap > MAX_CHAIN_GAP {
            return None;
        }
        // Догоняем цепочку до n; промежуточные msg keys уходят в skipped.
        let now = now_millis();
        let mut mk = None;
        for _ in 0..=gap {
            let (next, key) = chain.step();
            mk = Some(key);
            *chain = next;
            if chain.index <= n {
                self.skipped
                    .insert((remote_key, chain.index - 1), (key, now));
            }
        }
        self.skipped.remove(&(remote_key, n));
        self.evict_skipped_if_needed();
        aes_gcm_decrypt_aad(&mk?, &msg.ciphertext, &msg.nonce, &self.ad)
    }

    /// DH-ratchet шаг (первое сообщение уже обработано):
    /// 1) остаток recv-цепочки до pn -> skipped (индекс по СТАРОМУ dhr);
    /// 2) recv <- KDF(DH(текущий dhs, R));
    /// 3) свежий dhs; send <- KDF(DH(свежий dhs, R)); pn <- длина старой send.
    ///
    /// Обе стороны выполняют KDF в одном глобальном порядке, root не
    /// рассинхронизируется (парирующие DH: DH(our, their) == DH(their, our)).
    fn ratchet(&mut self, remote_key: [u8; 32], pn: u32) -> Option<()> {
        if let Some(chain) = self.recv_chain.as_mut() {
            let now = now_millis();
            while chain.index < pn {
                let (next, mk) = chain.step();
                self.skipped.insert((self.dhr?, chain.index), (mk, now));
                *chain = next;
                if self.skipped.len() > MAX_SKIPPED {
                    return None;
                }
            }
        }
        let dh_recv = dh_x_x(&self.dhs, &remote_key);
        let (root, ck) = root_kdf(&self.root_key, &dh_recv);
        self.root_key = root;
        self.recv_chain = Some(ChainKey { key: ck, index: 0 });

        let old_send_len = self.send_chain.map(|c| c.index).unwrap_or(0);
        let new_dhs = x25519_secret();
        let dh_send = dh_x_x(&new_dhs, &remote_key);
        let (root, send_ck) = root_kdf(&self.root_key, &dh_send);
        self.root_key = root;
        self.dhs = new_dhs;
        self.send_chain = Some(ChainKey {
            key: send_ck,
            index: 0,
        });
        self.dhr = Some(remote_key);
        self.prev_send_len = old_send_len;
        Some(())
    }

    /// Сколько msg keys сейчас в skipped (для тестов/диагностики).
    pub fn skipped_len(&self) -> usize {
        self.skipped.len()
    }

    /// Удаляет просроченные skipped keys.
    fn prune_skipped(&mut self) {
        let cutoff = now_millis().saturating_sub(SKIPPED_TTL_MS);
        self.skipped.retain(|_, (_, created)| *created > cutoff);
    }

    /// Eviction skipped до MAX_SKIPPED (FIFO по ключу карты — BTreeMap
    /// упорядочен, take() с начала отдаёт «старшие» индексы; для потолка
    /// 200 это приемлемая аппроксимация LRU).
    fn evict_skipped_if_needed(&mut self) {
        while self.skipped.len() > MAX_SKIPPED {
            let oldest = *self.skipped.keys().next().expect("non-empty");
            self.skipped.remove(&oldest);
        }
    }

    /// Eviction seen_ratchet_keys по LRU (last_seen) до SEEN_CAP.
    fn evict_seen_if_needed(&mut self) {
        while self.seen_ratchet_keys.len() > SEEN_CAP {
            let oldest = self
                .seen_ratchet_keys
                .iter()
                .min_by_key(|(_, ts)| *ts)
                .map(|(k, _)| *k)
                .expect("non-empty");
            self.seen_ratchet_keys.remove(&oldest);
        }
    }
}

/// Пропущенный msg key: ((ratchet_key пира, индекс), (msg_key, created_at)).
pub type SkippedEntry = (([u8; 32], u32), ([u8; 32], u64));
/// Виденный ratchet-ключ пира: (ключ, last_seen).
pub type SeenEntry = ([u8; 32], u64);

/// Сериализуемое состояние сессии (MUSH-A-SPEC §3). Versioned:
/// несовпадение version -> import None, не молчаливая потеря данных.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatchetSessionState {
    pub version: u32,
    pub root_key: [u8; 32],
    pub dhs: [u8; 32],
    pub dhr: Option<[u8; 32]>,
    pub send_chain: Option<([u8; 32], u32)>,
    pub recv_chain: Option<([u8; 32], u32)>,
    pub prev_send_len: u32,
    /// ((ratchet_key, index), (msg_key, created_at))
    pub skipped: Vec<SkippedEntry>,
    /// (ratchet_key, last_seen)
    pub seen: Vec<SeenEntry>,
    /// AD = IK_A‖IK_B — двумя полями: serde не умеет [u8; 64].
    pub ad_a: [u8; 32],
    pub ad_b: [u8; 32],
}

impl RatchetSession {
    /// Экспорт состояния: TTL-prune skipped и seen перед записью.
    /// Инвариант размера (< 8 KB) — в тестах.
    pub fn export_state(&mut self) -> RatchetSessionState {
        self.prune_skipped();
        let now = now_millis();
        let seen_cutoff = now.saturating_sub(SEEN_TTL_MS);
        self.seen_ratchet_keys.retain(|_, ts| *ts > seen_cutoff);
        self.evict_seen_if_needed();
        RatchetSessionState {
            version: 1,
            root_key: self.root_key,
            dhs: self.dhs.to_bytes(),
            dhr: self.dhr,
            send_chain: self.send_chain.map(|c| (c.key, c.index)),
            recv_chain: self.recv_chain.map(|c| (c.key, c.index)),
            prev_send_len: self.prev_send_len,
            skipped: self.skipped.iter().map(|(k, v)| (*k, *v)).collect(),
            seen: self
                .seen_ratchet_keys
                .iter()
                .map(|(k, ts)| (*k, *ts))
                .collect(),
            ad_a: self.ad[..32].try_into().expect("ad first half"),
            ad_b: self.ad[32..].try_into().expect("ad second half"),
        }
    }

    /// Восстановление состояния. `spk_secret` не сериализуется (transient):
    /// к моменту persist сессия уже обменялась сообщениями.
    pub fn import_state(state: &RatchetSessionState) -> Option<Self> {
        if state.version != 1 {
            return None;
        }
        let mut ad = [0u8; 64];
        ad[..32].copy_from_slice(&state.ad_a);
        ad[32..].copy_from_slice(&state.ad_b);
        Some(RatchetSession {
            root_key: state.root_key,
            dhs: StaticSecret::from(state.dhs),
            dhr: state.dhr,
            send_chain: state.send_chain.map(|(key, index)| ChainKey { key, index }),
            recv_chain: state.recv_chain.map(|(key, index)| ChainKey { key, index }),
            prev_send_len: state.prev_send_len,
            skipped: state.skipped.iter().map(|(k, v)| (*k, *v)).collect(),
            spk_secret: None,
            seen_ratchet_keys: state.seen.iter().map(|(k, ts)| (*k, *ts)).collect(),
            ad,
        })
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

    // ---- Double Ratchet ----

    /// Пара ratchet-сессий через настоящий X3DH.
    fn dr_pair() -> (RatchetSession, RatchetSession) {
        let (mut bob_store, _bob_id) = setup();
        let alice_id = Identity::generate();
        let bundle = bob_store.publish_bundle();
        let spk_public = bundle.spk;
        let (start, init) = X3dhInitiator::start(&alice_id, &bundle, b"hi").expect("x3dh");
        let (bob_start, _first) = bob_store.complete_handshake(&init).expect("x3dh resp");
        let bob_spk = bob_store.signed_pre_key_secret().clone();
        (
            RatchetSession::new_initiator(&start, &spk_public),
            RatchetSession::new_responder(&bob_start, bob_spk),
        )
    }

    #[test]
    fn dr_full_ping_pong_across_dh_ratchet() {
        let (mut alice, mut bob) = dr_pair();

        // Alice -> Bob: три сообщения по SK-цепочке.
        let msgs: Vec<_> = (0..3)
            .map(|i| alice.encrypt(format!("a{i}").as_bytes()).unwrap())
            .collect();
        for (i, m) in msgs.iter().enumerate() {
            assert_eq!(bob.decrypt(m).unwrap(), format!("a{i}").into_bytes());
        }

        // Bob -> Alice: ответ (первый ratchet-шаг Bob'а).
        let replies: Vec<_> = (0..2)
            .map(|i| bob.encrypt(format!("b{i}").as_bytes()).unwrap())
            .collect();
        for (i, m) in replies.iter().enumerate() {
            assert_eq!(alice.decrypt(m).unwrap(), format!("b{i}").into_bytes());
        }

        // Alice -> Bob: новая цепочка после её ratchet-шага.
        let more: Vec<_> = (0..2)
            .map(|i| alice.encrypt(format!("a2-{i}").as_bytes()).unwrap())
            .collect();
        for (i, m) in more.iter().enumerate() {
            assert_eq!(bob.decrypt(m).unwrap(), format!("a2-{i}").into_bytes());
        }
    }

    #[test]
    fn dr_responder_cannot_encrypt_before_first_message() {
        let (mut _alice, mut bob) = dr_pair();
        assert!(bob.encrypt(b"nope").is_none());
    }

    #[test]
    fn dr_out_of_order_within_chain() {
        let (mut alice, mut bob) = dr_pair();
        let msgs: Vec<_> = (0..4)
            .map(|i| alice.encrypt(format!("m{i}").as_bytes()).unwrap())
            .collect();
        assert_eq!(bob.decrypt(&msgs[0]).unwrap(), b"m0");
        assert_eq!(bob.decrypt(&msgs[2]).unwrap(), b"m2");
        assert!(bob.skipped_len() >= 1, "ключ m1 должен лежать в skipped");
        assert_eq!(bob.decrypt(&msgs[1]).unwrap(), b"m1");
        assert_eq!(bob.decrypt(&msgs[3]).unwrap(), b"m3");
    }

    #[test]
    fn dr_out_of_order_across_ratchet() {
        let (mut alice, mut bob) = dr_pair();
        // Alice шлёт a0, a1 по SK-цепочке; Bob видит только a0,
        // отвечает b0; Alice получает b0 и шлёт a2 УЖЕ по новой цепочке.
        let a0 = alice.encrypt(b"a0").unwrap();
        let a1 = alice.encrypt(b"a1").unwrap();
        assert_eq!(bob.decrypt(&a0).unwrap(), b"a0");
        let b0 = bob.encrypt(b"b0").unwrap();
        assert_eq!(alice.decrypt(&b0).unwrap(), b"b0");
        let a2 = alice.encrypt(b"a2").unwrap();

        // Bob первым получает a2 (новый ratchet, pn=2 должен спрятать a1),
        // потом доезжает опоздавшее a1.
        assert_eq!(bob.decrypt(&a2).unwrap(), b"a2");
        assert_eq!(bob.decrypt(&a1).unwrap(), b"a1");
    }

    #[test]
    fn dr_replay_rejected() {
        let (mut alice, mut bob) = dr_pair();
        let m0 = alice.encrypt(b"once").unwrap();
        assert_eq!(bob.decrypt(&m0).unwrap(), b"once");
        // Повтор того же сообщения: индекс уже пройден, skipped ключ
        // извлечён — дешифровки не будет.
        assert!(bob.decrypt(&m0).is_none());
    }

    #[test]
    fn dr_distinct_ciphertexts_and_gap_guard() {
        let (mut alice, mut bob) = dr_pair();
        let c0 = alice.encrypt(b"same").unwrap();
        let c1 = alice.encrypt(b"same").unwrap();
        assert_ne!(c0.ciphertext, c1.ciphertext);

        // Подделанный заголовок с недостижимым индексом — отказ.
        let mut far = c0.clone();
        far.header.chain_index = c1.header.chain_index + MAX_CHAIN_GAP + 1;
        assert!(bob.decrypt(&far).is_none());
    }

    #[test]
    fn dr_message_keys_forward_secret_within_chain() {
        let (mut alice, mut bob) = dr_pair();
        let m0 = alice.encrypt(b"secret-0").unwrap();
        let m1 = alice.encrypt(b"secret-1").unwrap();
        // Перехват m0 и его ключа не помогает: m1 шифруется другим msg key,
        // следующий msg key из перехваченного не выводится (HKDF-separation
        // ck/mk — mk не является входом следующего шага).
        assert_eq!(bob.decrypt(&m0).unwrap(), b"secret-0");
        assert_eq!(bob.decrypt(&m1).unwrap(), b"secret-1");
        // Структурная проверка: mk не порождает следующий mk.
        let chain = ChainKey {
            key: [7u8; 32],
            index: 0,
        };
        let (next_ck, mk0) = chain.step();
        let (_next2, mk1) = next_ck.step();
        // mk0 -> никакого шага не существует; next_ck известен только по chain key.
        assert_ne!(mk0, mk1);
        assert_ne!(mk0, next_ck.key);
    }

    // ---- Persistence (MUSH-A-SPEC §3) ----

    #[test]
    fn dr_export_import_continues_exchange() {
        let (mut alice, mut bob) = dr_pair();
        for i in 0..3 {
            let m = alice.encrypt(format!("pre-{i}").as_bytes()).unwrap();
            bob.decrypt(&m).unwrap();
        }
        let reply = bob.encrypt(b"reply").unwrap();
        alice.decrypt(&reply).unwrap();

        // Persist + restore Alice.
        let state = alice.export_state();
        let mut alice2 = RatchetSession::import_state(&state).expect("import");

        // Bob (не тронут) понимает сообщения восстановленной Alice.
        let after = alice2.encrypt(b"after-restore").unwrap();
        assert_eq!(bob.decrypt(&after).unwrap(), b"after-restore");
        // И обратно: восстановленная Alice понимает Bob'а (полный ratchet).
        let b2 = bob.encrypt(b"bob-again").unwrap();
        assert_eq!(alice2.decrypt(&b2).unwrap(), b"bob-again");
        let a3 = alice2.encrypt(b"alice-final").unwrap();
        assert_eq!(bob.decrypt(&a3).unwrap(), b"alice-final");
    }

    #[test]
    fn dr_import_rejects_unknown_version() {
        let (mut alice, _bob) = dr_pair();
        alice.encrypt(b"x").unwrap();
        let mut state = alice.export_state();
        state.version = 2;
        assert!(RatchetSession::import_state(&state).is_none());
    }

    #[test]
    fn dr_export_prunes_expired_skipped() {
        let (mut alice, mut bob) = dr_pair();
        let m0 = alice.encrypt(b"m0").unwrap();
        let _m1 = alice.encrypt(b"m1").unwrap();
        let m2 = alice.encrypt(b"m2").unwrap();
        bob.decrypt(&m0).unwrap();
        bob.decrypt(&m2).unwrap(); // m1 -> skipped (gap)
        assert!(bob.skipped_len() >= 1);
        // Подстареваем одну запись (имитация старости).
        let old = now_millis().saturating_sub(SKIPPED_TTL_MS + 1000);
        let key = *bob.skipped.keys().next().expect("skipped non-empty");
        bob.skipped.insert(key, ([9u8; 32], old));
        let state = bob.export_state();
        assert!(
            state.skipped.iter().all(|(_, (_, ts))| *ts > old),
            "просроченные skipped должны исчезнуть при export"
        );
    }

    #[test]
    fn dr_export_size_bounded_after_heavy_traffic() {
        let (mut alice, mut bob) = dr_pair();
        let mut pending = Vec::new();
        for i in 0..300u32 {
            let m = alice.encrypt(format!("msg-{i}").as_bytes()).unwrap();
            if i % 3 == 0 {
                // Каждое третье «теряется» до следующего ratchet -> skipped.
                pending.push(m);
            } else {
                bob.decrypt(&m).unwrap();
            }
            if i % 50 == 49 {
                // Регулярный ответ Bob'а — форсирует DH-ratchet шаги.
                let r = bob.encrypt(b"tick").unwrap();
                alice.decrypt(&r).unwrap();
            }
        }
        for m in pending.iter().take(50) {
            let _ = bob.decrypt(m); // часть может не дойти — неважно
        }
        let state = bob.export_state();
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&state, &mut cbor).expect("cbor");
        assert!(
            cbor.len() < 8192,
            "serialized state {} bytes exceeds 8 KB invariant",
            cbor.len()
        );
    }
}

#[cfg(test)]
mod golden {
    //! Golden vectors (MUSH-A-SPEC §2): пин KDF-примитивов. Любое изменение
    //! domain-строк, их порядка или структуры KDF ломает этот тест — защита
    //! от «зашитой» ошибки, которая молча протекла бы в persisted-сессии.
    use super::*;
    use crate::crypto::hkdf_derive;

    fn unhex(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn golden_kdf_primitives() {
        let ikm = [0x42u8; 32];
        assert_eq!(
            hkdf_derive(&ikm, b"mush-dr-ck", b""),
            unhex("461e035256c7336fe571f7dea33c0271119883560f945c891f5c221a42a4ef89")
        );
        assert_eq!(
            hkdf_derive(&ikm, b"mush-dr-mk", b""),
            unhex("a82c7f766251ee4728da3e4be268116dac3cef0035967d1217d8c799d64643f8")
        );
        assert_eq!(
            hkdf_derive(&ikm, b"mush-dr-root", b""),
            unhex("6f31c3a0d3398d2448f587caac35cc445f9e76c55c4d2bd346e5b44330ce8765")
        );
        assert_eq!(
            hkdf_derive(&ikm, b"mush-dr-chain", b""),
            unhex("3107de13107cff441c377dbd155f45448119228c104ef394ea4a02d2257d1bd8")
        );
    }

    #[test]
    fn golden_x3dh_and_root_and_chain_step() {
        let ad = associated_data(&[0x11u8; 32], &[0x22u8; 32]);
        assert_eq!(
            derive_sk(&ad, &[[1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]]),
            unhex("3ff7e8e05efcf3e5eeec70a002addab29742f17c7bc0f806338f5c6034088395")
        );
        let (r, c) = root_kdf(&[0x42u8; 32], &[0x55u8; 32]);
        assert_eq!(
            r,
            unhex("34f5bbb5f29173add257a992bed91cb8adc5bba47a0841269a9cab2ecb0d3140")
        );
        assert_eq!(
            c,
            unhex("17eba4930a9efd3f2fef8b52e2713ec17f3375f85b9f6a58572505882c2cd18e")
        );
        let chain = ChainKey {
            key: [0x42u8; 32],
            index: 0,
        };
        let (next, mk) = chain.step();
        assert_eq!(
            next.key,
            unhex("461e035256c7336fe571f7dea33c0271119883560f945c891f5c221a42a4ef89")
        );
        assert_eq!(
            mk,
            unhex("a82c7f766251ee4728da3e4be268116dac3cef0035967d1217d8c799d64643f8")
        );
        assert_eq!(next.index, 1);
    }
}
