use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;

use crate::base64;
use crate::crdt::{hex, AdHocGroup, ChatMessage, GroupId, PubKey};
use crate::crypto::ratchet;
use crate::framing::{
    self, Frame, TYPE_DECOY, TYPE_DECOY_PUBLISH, TYPE_HANDSHAKE, TYPE_RATCHET, TYPE_SYNC,
    TYPE_TEXT, TYPE_X3DH_INIT,
};
use crate::identity::Identity;
use crate::sensors::{
    now_millis, Alert, ContactRateSensor, DecoyEidSensor, InboxBuffer, IncomingMessage,
};
use crate::sns::{
    HandshakeError, HandshakeInitiator, HandshakeMessage, HandshakeResponder, SnsSession, NONCE_TTL,
};
use rand::RngCore;

/// One confirmed peer connection.
struct Peer {
    writer: OwnedWriteHalf,
    name: String,
    decoys: Vec<[u8; 32]>,
    #[allow(dead_code)] // established per A'-SPEC §3.1; chat payload uses group_key in A'
    session_key: [u8; 32],
    group_id: GroupId,
    /// Этап A, ratchet-режим: pairwise Double Ratchet сессия с этим пиром.
    ratchet: Option<crate::crypto::ratchet::RatchetSession>,
    /// Этап A: получили ли хотя бы один ratchet-фрейм от этого пира.
    /// Пока false — к каждому исходящему фрейму подмешивается активация
    /// (восстановление при потере первой активации в DTN, MUSH-A-SPEC §4.0.1).
    seen_incoming: bool,
}

/// Known contact (CLI-SPEC: /contacts).
#[derive(Debug, Clone)]
pub struct Contact {
    pub name: String,
    pub last_seen: Option<Instant>,
}

/// Invite-link (A'-SPEC §7.2, CLI adaptation): carries pubkey, one-time nonce,
/// rendezvous address, the ad-hoc group material that Android puts in QR,
/// и (этап A, ratchet-режим) pre-key bundle в поле `pk=`.
#[derive(Debug, Clone)]
pub struct Invite {
    pub pubkey: PubKey,
    pub nonce: [u8; 16],
    pub addr: String,
    pub group_id: GroupId,
    pub group_key: [u8; 32],
    /// base64(CBOR(PreKeyBundle)) — small manifest, ≤512 символов.
    pub pk: Option<String>,
    /// Отображаемое имя владельца invite (base64 в render). Неаутентифицировано
    /// (invite — сам доверенный носитель через QR); нужно ratchet-пути, где
    /// у initiator'а иначе нет способа узнать имя responder'а — Legacy-путь
    /// получает имя из подписанного handshake-reply и это поле игнорирует.
    pub name: Option<String>,
}

impl Invite {
    pub fn render(&self) -> String {
        let base = format!(
            "mush://{}&nonce={}&addr={}&group={}-{}",
            base64::encode(&self.pubkey),
            base64::encode(&self.nonce),
            self.addr,
            base64::encode(&self.group_id),
            base64::encode(&self.group_key),
        );
        let with_pk = match &self.pk {
            Some(pk) => format!("{base}&pk={pk}"),
            None => base,
        };
        match &self.name {
            Some(name) => format!("{}&name={}", with_pk, base64::encode(name.as_bytes())),
            None => with_pk,
        }
    }

    pub fn parse(s: &str) -> Result<Invite, String> {
        let body = s
            .trim()
            .strip_prefix("mush://")
            .ok_or("not a mush:// link")?;
        // First segment (before any '&') is the bare pubkey base64 — it must be
        // split off BEFORE the key=value parsing because base64 padding is '='.
        let (pk_part, query) = match body.split_once('&') {
            Some((head, rest)) => (head, rest),
            None => (body, ""),
        };
        let pk_bytes = base64::decode(pk_part).ok_or("bad pubkey base64")?;
        let pubkey =
            <[u8; 32]>::try_from(pk_bytes.as_slice()).map_err(|_| "pubkey must be 32 bytes")?;

        let mut nonce = None;
        let mut addr = None;
        let mut group = None;
        let mut pk = None;
        let mut name = None;
        for part in query.split('&') {
            let Some((key, val)) = part.split_once('=') else {
                continue;
            };
            match key {
                "nonce" => {
                    let bytes = base64::decode(val).ok_or("bad nonce base64")?;
                    nonce = Some(
                        <[u8; 16]>::try_from(bytes.as_slice())
                            .map_err(|_| "nonce must be 16 bytes")?,
                    );
                }
                "addr" => addr = Some(val.to_string()),
                "pk" => pk = Some(val.to_string()),
                "name" => {
                    let bytes = base64::decode(val).ok_or("bad name base64")?;
                    name = Some(String::from_utf8(bytes).map_err(|_| "name not utf-8")?);
                }
                "group" => {
                    let (gid_b64, gkey_b64) = val.split_once('-').ok_or("bad group part")?;
                    let gid = base64::decode(gid_b64).ok_or("bad group id base64")?;
                    let gkey = base64::decode(gkey_b64).ok_or("bad group key base64")?;
                    group = Some((
                        <[u8; 32]>::try_from(gid.as_slice())
                            .map_err(|_| "group id must be 32 bytes")?,
                        <[u8; 32]>::try_from(gkey.as_slice())
                            .map_err(|_| "group key must be 32 bytes")?,
                    ));
                }
                // Tolerant parsing: неизвестные поля invite игнорируются
                // (форвард-совместимость, MUSH-A-SPEC §4.1).
                _ => {}
            }
        }
        let (group_id, group_key) = group.ok_or("invite missing group")?;
        Ok(Invite {
            pubkey,
            nonce: nonce.ok_or("invite missing nonce")?,
            addr: addr.ok_or("invite missing addr")?,
            group_id,
            group_key,
            pk,
            name,
        })
    }
}

/// Sync frame payload (0x02): a request carries `max_lamport`, a reply carries
/// the missing `messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub max_lamport: Option<u64>,
    pub messages: Option<Vec<ChatMessage>>,
}

/// Ratchet frame payload (0x07, этап A): открытые CRDT-поля (id, lamport)
/// плюс pairwise Double Ratchet сообщение. Текст едет внутри ratchet AEAD.
/// Поля id/lamport открыты — в legacy-режиме они тоже открыты (ChatMessage
/// курсирует по фрейму), а для CRDT-мержа нужны всем сторонам.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatchetFrame {
    pub id: String,
    pub lamport: u64,
    pub message: ratchet::RatchetMessage,
}

/// Core-level node events. Emitted over an optional sink so embedders (the JNI
/// bridge) get push notifications instead of polling; the CLI leaves the sink
/// unset. No JNI types here — plain data, protocol unchanged.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// A new chat message was accepted into a group's history (inbound path;
    /// the sender's own echo is not an event).
    Message {
        group: GroupId,
        sender: PubKey,
        text: String,
        lamport: u64,
    },
    /// A sensor alert surfaced to the user.
    Alert(Alert),
    /// Handshake with a peer completed (inbound or outbound).
    HandshakeComplete { peer: PubKey, name: String },
}

/// Decoy frame payload (0x04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoyPayload {
    pub destination: [u8; 32],
}

/// Публикация активных decoy-EID (0x08): ratchet-путь заменяет decoy-поле
/// SNS handshake. Принимающая сторона кладёт список в `Peer.decoys` —
/// /decoy работает в обоих режимах.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoyPublishPayload {
    pub decoys: Vec<[u8; 32]>,
}

/// Node state + behavior (CLI-SPEC §2.1). One node = one process.
pub struct Node {
    identity: Identity,
    name: String,
    configured_port: u16,
    bound_addr: Option<String>,
    /// Address override for invites: the IP a peer should dial. Embedders set
    /// this when the listener binds 0.0.0.0 but the reachable address is a
    /// specific interface (Android hotspot/LAN). CLI leaves it unset.
    advertised_addr: Option<String>,
    pub contacts: HashMap<PubKey, Contact>,
    pub groups: HashMap<GroupId, AdHocGroup>,
    pub inbox: InboxBuffer,
    s1: ContactRateSensor,
    s7: DecoyEidSensor,
    alerts: Vec<(u128, Alert)>,
    issued_nonces: HashMap<[u8; 16], Instant>,
    peers: HashMap<PubKey, Peer>,
    pending_bursts: HashMap<PubKey, (usize, Instant)>,
    pub flush_every: Duration,
    pub idle_flush: Duration,
    /// Handle to ourselves so outbound joins can spawn their read loop.
    self_ref: Option<Arc<Mutex<Node>>>,
    /// Optional push-event sink (see `NodeEvent`). Set by embedders; the CLI
    /// and tests leave it unset.
    event_sink: Option<mpsc::UnboundedSender<NodeEvent>>,
    /// Этап A: протокол рукопожатия/сообщений. Default Ratchet — основной
    /// режим с этапа B (MUSH-B-SPEC §0: Legacy deprecated, жив до этапа C);
    /// Legacy включается явно через `set_crypto_mode` / CLI `--crypto legacy`.
    crypto_mode: CryptoMode,
    /// Этап A: pre-keys (SPK + OPK) для ratchet-режима.
    pre_key_store: ratchet::PreKeyStore,
    /// Этап A: tombstone использованных бандлов на стороне initiator'а.
    bundle_consumer: ratchet::BundleConsumer,
    /// Этап A: отправлять ли активацию сразу после join (default true).
    /// Тесты отключают, чтобы эмулировать потерю активации в DTN —
    /// восстановление обязано произойти через folding в send_ratchet_to.
    activation_enabled: bool,
    /// Этап B: ratchet-сессии, восстановленные из persist (import_state).
    /// Peers — это живые TCP-соединения (writer), их нельзя восстановить
    /// без сети: сессии хранятся отдельно, пока будущий transport/vault-код
    /// не решит поднимать соединение (MUSH-B-SPEC §3).
    restored_sessions: HashMap<PubKey, ratchet::RatchetSession>,
}

/// Протокол обмена (MUSH-A-SPEC §5): Legacy — SNS handshake + group_key
/// (A'-SPEC §3.1); Ratchet — X3DH + Double Ratchet pairwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoMode {
    Legacy,
    Ratchet,
}

/// Сериализуемое состояние узла (persist, MUSH-B-SPEC §3). Versioned:
/// несовпадение version -> import None. Живые TCP-writer'ы и last_seen
/// (Instant) в persist не входят — восстанавливаются только сессии.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub version: u32,
    pub name: String,
    /// "legacy" | "ratchet" (строка — устойчива к рефакторингу enum'а).
    pub crypto_mode: String,
    pub identity_seed: [u8; 32],
    pub pre_key_store: ratchet::PreKeyStoreState,
    pub bundle_consumer: ratchet::BundleConsumerState,
    /// Peer-сессии из активных peers с ratchet (pubkey, состояние).
    pub peer_sessions: Vec<(PubKey, ratchet::RatchetSessionState)>,
    /// Контакты: (pubkey, имя). last_seen не сериализуется.
    pub contacts: Vec<(PubKey, String)>,
}

impl Node {
    pub fn new(name: &str, port: u16) -> Node {
        let identity = Identity::generate();
        let me = identity.pubkey_bytes();
        let mut node = Node {
            identity: identity.clone(),
            name: name.to_string(),
            configured_port: port,
            bound_addr: None,
            advertised_addr: None,
            contacts: HashMap::new(),
            groups: HashMap::new(),
            inbox: InboxBuffer::new(),
            s1: ContactRateSensor::new(),
            s7: DecoyEidSensor::new(me),
            alerts: Vec::new(),
            issued_nonces: HashMap::new(),
            peers: HashMap::new(),
            pending_bursts: HashMap::new(),
            flush_every: Duration::from_millis(500),
            idle_flush: Duration::from_secs(1),
            self_ref: None,
            event_sink: None,
            crypto_mode: CryptoMode::Ratchet,
            pre_key_store: ratchet::PreKeyStore::new(identity),
            bundle_consumer: ratchet::BundleConsumer::new(),
            activation_enabled: true,
            restored_sessions: HashMap::new(),
        };
        // Дефолт — Ratchet: узел сразу готов к X3DH (очередь OPK пополнена
        // до того же уровня, что и при `set_crypto_mode(Ratchet)`).
        if node.pre_key_store.pending_count() < 50 {
            node.pre_key_store.replenish(50);
        }
        node
    }

    /// Этап A (тесты): включить/выключить немедленную активацию после join.
    pub fn set_activation_enabled(&mut self, enabled: bool) {
        self.activation_enabled = enabled;
    }

    /// Этап A: включить ratchet-режим (до `run`/первого invite). Пополняет
    /// очередь одноразовых ключей.
    pub fn set_crypto_mode(&mut self, mode: CryptoMode) {
        self.crypto_mode = mode;
        if mode == CryptoMode::Ratchet && self.pre_key_store.pending_count() < 50 {
            self.pre_key_store.replenish(50);
        }
    }

    pub fn crypto_mode(&self) -> CryptoMode {
        self.crypto_mode
    }

    /// Этап A: responder-side приём X3dhInit (только ratchet-режим).
    pub fn accept_ratchet(
        &mut self,
        init: &ratchet::X3dhInit,
    ) -> Option<(ratchet::SessionStart, Vec<u8>)> {
        if self.crypto_mode != CryptoMode::Ratchet {
            return None;
        }
        self.pre_key_store.complete_handshake(init)
    }

    /// Этап A: экспорт pairwise-сессии с пиром (persist, &self — идемпотентно).
    pub fn export_peer_session(&self, peer: &PubKey) -> Option<ratchet::RatchetSessionState> {
        self.peers
            .get(peer)?
            .ratchet
            .as_ref()
            .map(|s| s.export_state())
    }

    /// Этап A: восстановление pairwise-сессии после рестарта.
    pub fn import_peer_session(
        &mut self,
        peer: &PubKey,
        state: &ratchet::RatchetSessionState,
    ) -> bool {
        let Some(session) = ratchet::RatchetSession::import_state(state) else {
            return false;
        };
        match self.peers.get_mut(peer) {
            Some(p) => {
                p.ratchet = Some(session);
                true
            }
            None => false,
        }
    }

    /// Этап B: сессии, восстановленные из persist (import_state). Смотри
    /// поле `restored_sessions` — активные TCP-соединения этим не трогаются.
    pub fn restored_session(&self, peer: &PubKey) -> Option<&ratchet::RatchetSession> {
        self.restored_sessions.get(peer)
    }

    /// Извлечь восстановленную сессию (вызыватель решает, когда поднимать
    /// соединение). Второй вызов вернёт None — извлечение одноразово.
    pub fn take_restored_session(&mut self, peer: &PubKey) -> Option<ratchet::RatchetSession> {
        self.restored_sessions.remove(peer)
    }

    /// Этап B: экспорт узла в persist-форму (MUSH-B-SPEC §3). Versioned.
    /// Контакты — pubkey+имя; last_seen (Instant) и живые TCP-writer
    /// не сериализуются.
    pub fn export_state(&self) -> NodeState {
        let peer_sessions: Vec<(PubKey, ratchet::RatchetSessionState)> = self
            .peers
            .iter()
            .filter_map(|(pk, peer)| peer.ratchet.as_ref().map(|s| (*pk, s.export_state())))
            .collect();
        let mut contacts: Vec<(PubKey, String)> = self
            .contacts
            .iter()
            .map(|(pk, c)| (*pk, c.name.clone()))
            .collect();
        contacts.sort_by_key(|c| c.0);
        NodeState {
            version: 1,
            name: self.name.clone(),
            crypto_mode: match self.crypto_mode {
                CryptoMode::Legacy => "legacy".to_string(),
                CryptoMode::Ratchet => "ratchet".to_string(),
            },
            identity_seed: self.identity.seed(),
            pre_key_store: self.pre_key_store.export_state(),
            bundle_consumer: self.bundle_consumer.export_state(),
            peer_sessions,
            contacts,
        }
    }

    /// Этап B: восстановление узла из persist. Восстановленные ratchet-сессии
    /// кладутся в `restored_sessions` (не в peers — peers живут, пока живо
    /// TCP-соединение). None при version != 1 или порче вложенного state.
    pub fn import_state(state: &NodeState) -> Option<Node> {
        if state.version != 1 {
            return None;
        }
        let crypto_mode = match state.crypto_mode.as_str() {
            "legacy" => CryptoMode::Legacy,
            "ratchet" => CryptoMode::Ratchet,
            _ => return None,
        };
        let mut node = Node::new(&state.name, 0); // пополнит OPK...
        let identity = Identity::from_seed(state.identity_seed);
        node.identity = identity.clone();
        node.s7 = crate::sensors::DecoyEidSensor::new(identity.pubkey_bytes());
        node.crypto_mode = crypto_mode;
        node.pre_key_store = ratchet::PreKeyStore::import_state(&state.pre_key_store)?; // ...store перезаписан
        node.bundle_consumer = ratchet::BundleConsumer::import_state(&state.bundle_consumer)?;
        for (pk, session_state) in &state.peer_sessions {
            let session = ratchet::RatchetSession::import_state(session_state)?;
            node.restored_sessions.insert(*pk, session);
        }
        for (pk, name) in &state.contacts {
            node.contacts.insert(
                *pk,
                Contact {
                    name: name.clone(),
                    last_seen: None,
                },
            );
        }
        Some(node)
    }

    /// Attach a push-event sink. Emission is fire-and-forget: if the receiver
    /// is gone, events are dropped silently.
    pub fn set_event_sink(&mut self, sink: mpsc::UnboundedSender<NodeEvent>) {
        self.event_sink = Some(sink);
    }

    fn emit(&self, event: NodeEvent) {
        if let Some(sink) = &self.event_sink {
            let _ = sink.send(event);
        }
    }

    /// Store a handle to the shared node (set by `run` before loops start).
    pub fn set_self_ref(&mut self, me: Arc<Mutex<Node>>) {
        self.self_ref = Some(me);
    }

    pub fn pubkey(&self) -> PubKey {
        self.identity.pubkey_bytes()
    }

    pub fn set_bound(&mut self, addr: std::net::SocketAddr) {
        self.bound_addr = Some(addr.to_string());
    }

    /// Override the address invites carry (reachable interface IP). `None`
    /// restores the default (bound addr, or 127.0.0.1 fallback).
    pub fn set_advertised_addr(&mut self, addr: Option<String>) {
        self.advertised_addr = addr;
    }

    /// Mint a fresh one-time invite (new nonce, TTL 24h). Creates the node's
    /// default group lazily on first use (creator flow).
    pub fn invite_now(&mut self) -> String {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        self.issued_nonces.insert(nonce, Instant::now());
        let addr = self.advertised_addr.clone().unwrap_or_else(|| {
            self.bound_addr
                .clone()
                .unwrap_or_else(|| format!("127.0.0.1:{}", self.configured_port))
        });
        let group_id = self.ensure_default_group();
        let group = &self.groups[&group_id];
        // Ratchet-режим: к invite прикрепляем свежий pre-key bundle (pk=),
        // сканирующий получает и группу, и бандл одной строкой.
        let pk = if self.crypto_mode == CryptoMode::Ratchet {
            if self.pre_key_store.pending_count() < 10 {
                self.pre_key_store.replenish(50);
            }
            self.pre_key_store.publish_bundle().to_invite_field()
        } else {
            None
        };
        Invite {
            pubkey: self.pubkey(),
            nonce,
            addr,
            group_id: group.group_id,
            group_key: group.group_key,
            pk,
            name: Some(self.name.clone()),
        }
        .render()
    }

    /// The group's working group: created on demand, replaced by the joined
    /// group on `/join` so a fresh node has exactly one meaningful group.
    fn ensure_default_group(&mut self) -> GroupId {
        if let Some(id) = self.groups.keys().next().copied() {
            return id;
        }
        let me = self.pubkey();
        let group = AdHocGroup::create(&me);
        let id = group.group_id;
        self.groups.insert(id, group);
        id
    }

    fn purge_invites(&mut self) {
        let cutoff = Instant::now()
            .checked_sub(NONCE_TTL)
            .unwrap_or_else(Instant::now);
        self.issued_nonces.retain(|_, at| *at > cutoff);
    }

    fn resolve_group(&self, prefix: &str) -> Option<GroupId> {
        let prefix = prefix.trim_start_matches("group-");
        if prefix.is_empty() || prefix == "default" {
            // The working group: the joined group if there is exactly one with
            // content, otherwise the only group. A node that joined drops its
            // empty auto-created group (see join_peer), keeping this unambiguous.
            return self
                .groups
                .keys()
                .next()
                .copied()
                .filter(|_| self.groups.len() == 1);
        }
        let mut found = self.groups.keys().filter(|&g| hex(g).starts_with(prefix));
        let first = found.next()?;
        if found.next().is_some() {
            None // ambiguous
        } else {
            Some(*first)
        }
    }

    fn group_label(group_id: &GroupId) -> String {
        format!("group-{}", &hex(group_id)[..6])
    }

    fn contact_name(&self, key: &PubKey) -> String {
        self.contacts
            .get(key)
            .map(|c| c.name.clone())
            .or_else(|| self.peers.get(key).map(|p| p.name.clone()))
            .unwrap_or_else(|| crate::sensors::short_key(key))
    }

    fn push_alert(&mut self, alert: Alert) {
        tracing::warn!("{}", alert.render());
        self.emit(NodeEvent::Alert(alert.clone()));
        self.alerts.push((now_millis(), alert));
    }

    /// Register a completed handshake on this node.
    fn register_peer(
        &mut self,
        session: SnsSession,
        writer: OwnedWriteHalf,
        decoys: Vec<[u8; 32]>,
        group_id: GroupId,
    ) {
        self.contacts.insert(
            session.peer_pubkey,
            Contact {
                name: session.peer_name.clone(),
                last_seen: Some(Instant::now()),
            },
        );
        if let Some(group) = self.groups.get_mut(&group_id) {
            group.add_member(session.peer_pubkey);
        }
        self.peers.insert(
            session.peer_pubkey,
            Peer {
                writer,
                name: session.peer_name.clone(),
                decoys,
                session_key: session.session_key,
                group_id,
                ratchet: None,
                seen_incoming: false,
            },
        );
        self.emit(NodeEvent::HandshakeComplete {
            peer: session.peer_pubkey,
            name: session.peer_name.clone(),
        });
        tracing::info!(
            "Handshake complete with {} ({}). Session key established.",
            session.peer_name,
            crate::sensors::short_key(&session.peer_pubkey),
        );
    }

    /// Inbound handshake (accepted connection). Returns the reply to send.
    fn accept_handshake(
        &mut self,
        offer: &HandshakeMessage,
    ) -> Result<(HandshakeMessage, SnsSession), HandshakeError> {
        self.purge_invites();
        if !self.issued_nonces.contains_key(&offer.nonce) {
            return Err(HandshakeError::NonceMismatch);
        }
        let mut responder = HandshakeResponder::new(self.identity.clone(), Some(offer.nonce));
        let (reply, session) =
            responder.respond(offer, &self.name, self.s7.active_decoys().to_vec())?;
        // One-time token: consumed on successful handshake.
        self.issued_nonces.remove(&offer.nonce);
        Ok((reply, session))
    }

    /// Outbound join: parse invite, connect, handshake, register peer, join group.
    pub async fn join_peer(&mut self, invite_str: &str) -> Result<(), String> {
        let invite = Invite::parse(invite_str)?;
        let stream = timeout(Duration::from_secs(5), TcpStream::connect(&invite.addr))
            .await
            .map_err(|_| "connect timed out")?
            .map_err(|e| format!("connect to {} failed: {e}", invite.addr))?;
        let (mut reader, writer) = stream.into_split();
        let mut writer = writer;

        // Этап A, ratchet-режим: X3DH вместо SNS. Первый фрейм — X3dhInit.
        if self.crypto_mode == CryptoMode::Ratchet {
            let pk_field = invite
                .pk
                .as_deref()
                .ok_or("invite missing pk= bundle (ratchet mode)")?;
            let bundle =
                ratchet::PreKeyBundle::from_invite_field(pk_field).ok_or("bad pk= bundle")?;
            if !self.bundle_consumer.mark_used(&bundle) {
                return Err("bundle OPK already used by this node".to_string());
            }
            let (start, init) =
                ratchet::X3dhInitiator::start(&self.identity, &bundle, self.name.as_bytes())
                    .ok_or("x3dh initiator failed")?;
            let frame = Frame::typed(TYPE_X3DH_INIT, &init).map_err(|e| e.to_string())?;
            framing::write_frame(&mut writer, &frame)
                .await
                .map_err(|e| format!("send x3dh init failed: {e}"))?;

            if !self.groups.contains_key(&invite.group_id) {
                let me = self.pubkey();
                self.groups.insert(
                    invite.group_id,
                    AdHocGroup::join(invite.group_id, invite.group_key, &me),
                );
                let dead: Vec<GroupId> = self
                    .groups
                    .iter()
                    .filter(|(id, g)| {
                        **id != invite.group_id && g.is_empty() && g.members.len() <= 1
                    })
                    .map(|(id, _)| *id)
                    .collect();
                for id in dead {
                    self.groups.remove(&id);
                }
            }
            let peer_key = bundle.identity;
            // Имя responder'а — из invite (поле `name=`; неаутентифицировано,
            // но invite и есть доверенный носитель — QR). Fallback — short key.
            let owner_name = invite
                .name
                .clone()
                .unwrap_or_else(|| crate::sensors::short_key(&peer_key));
            self.contacts.insert(
                peer_key,
                Contact {
                    name: owner_name.clone(),
                    last_seen: Some(Instant::now()),
                },
            );
            if let Some(group) = self.groups.get_mut(&invite.group_id) {
                group.add_member(peer_key);
            }
            self.peers.insert(
                peer_key,
                Peer {
                    writer,
                    name: owner_name.clone(),
                    decoys: Vec::new(),
                    session_key: [0u8; 32],
                    group_id: invite.group_id,
                    ratchet: Some(ratchet::RatchetSession::new_initiator(&start, &bundle.spk)),
                    seen_incoming: false,
                },
            );
            let me = self
                .self_ref
                .clone()
                .ok_or("node is not running (no self handle)")?;
            tokio::spawn(async move {
                Node::outbound_loop(me, peer_key, reader).await;
            });
            // Parity с Legacy-путём: handshake-completion виден в логах и
            // событиях на обеих сторонах (MUSH-B-SPEC §0: ratchet — основной
            // режим, smoke и JNI-тесты завязаны на эти строки).
            self.emit(NodeEvent::HandshakeComplete {
                peer: peer_key,
                name: owner_name.clone(),
            });
            tracing::info!(
                "Handshake complete with {} ({}). Session key established.",
                owner_name,
                crate::sensors::short_key(&peer_key),
            );
            tracing::info!("Group joined: {}", Self::group_label(&invite.group_id));
            self.publish_decoys_to(&peer_key).await;
            // Активация: первое ratchet-сообщение (пустой текст) владельцу —
            // без него responder-сторона не может шифровать (send-цепочка
            // строится из DH с ratchet-ключом initiator'а). При потере в DTN
            // восстановление — folding в send_ratchet_to (§4.0.1).
            if self.activation_enabled {
                let activation = {
                    let peer = self.peers.get_mut(&peer_key).expect("peer just inserted");
                    peer.ratchet.as_mut().and_then(|s| s.encrypt(b""))
                };
                if let Some(msg) = activation {
                    if let Ok(frame) = Frame::typed(
                        TYPE_RATCHET,
                        &RatchetFrame {
                            id: "activation".to_string(),
                            lamport: 0,
                            message: msg,
                        },
                    ) {
                        self.send_to(&peer_key, &frame).await;
                    }
                }
            }
            return Ok(());
        }

        let initiator = HandshakeInitiator::new(self.identity.clone(), invite.nonce);
        let offer = initiator.start(&self.name, self.s7.active_decoys().to_vec());
        let frame = Frame::typed(TYPE_HANDSHAKE, &offer).map_err(|e| e.to_string())?;
        framing::write_frame(&mut writer, &frame)
            .await
            .map_err(|e| format!("send handshake failed: {e}"))?;

        let reply_frame = timeout(Duration::from_secs(10), framing::read_frame(&mut reader))
            .await
            .map_err(|_| "handshake reply timed out")?
            .map_err(|e| format!("read handshake reply failed: {e}"))?
            .ok_or("connection closed during handshake")?;
        if reply_frame.msg_type != TYPE_HANDSHAKE {
            return Err("expected handshake reply".into());
        }
        let reply: HandshakeMessage = reply_frame.decode().map_err(|e| e.to_string())?;
        let session = initiator.finish(&reply).map_err(|e| e.to_string())?;

        if !self.groups.contains_key(&invite.group_id) {
            let me = self.pubkey();
            self.groups.insert(
                invite.group_id,
                AdHocGroup::join(invite.group_id, invite.group_key, &me),
            );
            // The joined group becomes the working group: drop our own
            // auto-created group if it is still empty (no messages, only us).
            let dead: Vec<GroupId> = self
                .groups
                .iter()
                .filter(|(id, g)| **id != invite.group_id && g.is_empty() && g.members.len() <= 1)
                .map(|(id, _)| *id)
                .collect();
            for id in dead {
                self.groups.remove(&id);
            }
        }
        let peer_key = session.peer_pubkey;
        self.register_peer(session, writer, reply.decoys.clone(), invite.group_id);
        // Spawn the read loop for this outbound connection (sync replies, relays).
        let me = self
            .self_ref
            .clone()
            .ok_or("node is not running (no self handle)")?;
        tokio::spawn(async move {
            Node::outbound_loop(me, peer_key, reader).await;
        });
        tracing::info!("Group joined: {}", Self::group_label(&invite.group_id));
        Ok(())
    }

    /// Handle one inbound frame from a confirmed peer connection.
    pub async fn process_frame(&mut self, conn: &PubKey, frame: &Frame) {
        self.contacts
            .entry(*conn)
            .and_modify(|c| c.last_seen = Some(Instant::now()));
        match frame.msg_type {
            TYPE_TEXT => {
                let Ok(msg) = frame.decode::<ChatMessage>() else {
                    tracing::warn!(
                        "undecodable text frame from {}",
                        crate::sensors::short_key(conn)
                    );
                    return;
                };
                let Some(group_id) = self.peers.get(conn).map(|p| p.group_id) else {
                    return;
                };
                let is_new = match self.groups.get_mut(&group_id) {
                    Some(group) => group.insert(msg.clone()),
                    None => false,
                };
                if !is_new {
                    return; // duplicate — stop relay
                }
                let (text, group_label) = match self.groups.get(&group_id) {
                    Some(group) => (
                        group
                            .decrypt(&msg)
                            .unwrap_or_else(|| "<undecryptable>".into()),
                        Self::group_label(&group_id),
                    ),
                    None => return,
                };
                self.emit(NodeEvent::Message {
                    group: group_id,
                    sender: msg.sender,
                    text: text.clone(),
                    lamport: msg.timestamp,
                });
                println!("[{group_label}] {}: {text}", self.contact_name(&msg.sender));
                self.inbox.push(IncomingMessage {
                    from: msg.sender,
                    text,
                    received_at: std::time::SystemTime::now(),
                });
                let entry = self
                    .pending_bursts
                    .entry(msg.sender)
                    .or_insert((0, Instant::now()));
                entry.0 += 1;
                entry.1 = Instant::now();
                self.relay(conn, frame).await;
            }
            TYPE_RATCHET => {
                // Этап A: pairwise ratchet-сообщение. Текст — внутри AEAD,
                // id/lamport открыты для CRDT-мержа; content группы хранит
                // plaintext (sender-side история — group_key AES: читаем
                // оба варианта, см. history_texts).
                if self.crypto_mode != CryptoMode::Ratchet {
                    tracing::warn!("ratchet frame in legacy mode — dropped");
                    return;
                }
                let Ok(rf) = frame.decode::<RatchetFrame>() else {
                    return;
                };
                let (text, group_id, sender) = {
                    let Some(peer) = self.peers.get_mut(conn) else {
                        return;
                    };
                    let Some(session) = peer.ratchet.as_mut() else {
                        return;
                    };
                    let Some(bytes) = session.decrypt(&rf.message) else {
                        tracing::warn!(
                            "ratchet decrypt failed from {}",
                            crate::sensors::short_key(conn)
                        );
                        return;
                    };
                    // Первый успешно открытый фрейм пира — активация его
                    // send-цепочки с нашей стороны больше не нужна.
                    peer.seen_incoming = true;
                    (
                        String::from_utf8_lossy(&bytes).into_owned(),
                        peer.group_id,
                        *conn,
                    )
                };
                // Пустой текст — активация сессии (initiator -> responder
                // сразу после join): активирует send-цепочку responder'а,
                // не плодя записи в истории.
                if text.is_empty() {
                    return;
                }
                let msg = ChatMessage {
                    id: rf.id,
                    sender,
                    content: text.clone().into_bytes(),
                    timestamp: rf.lamport,
                    nonce: [0u8; 12],
                };
                let is_new = match self.groups.get_mut(&group_id) {
                    Some(group) => group.insert(msg.clone()),
                    None => false,
                };
                if !is_new {
                    return; // duplicate — stop relay
                }
                let group_label = Self::group_label(&group_id);
                self.emit(NodeEvent::Message {
                    group: group_id,
                    sender: msg.sender,
                    text: text.clone(),
                    lamport: msg.timestamp,
                });
                println!("[{group_label}] {}: {text}", self.contact_name(&msg.sender));
                // Pairwise-шифрование не ретранслирует сырой фрейм:
                // релей-узел расшифровал — перешифровывает для каждого
                // пира СВОЕЙ сессией (те же id/lamport -> CRDT-дедуп).
                self.relay_ratchet(conn, group_id, &msg.id, msg.timestamp, text.as_str())
                    .await;
                self.inbox.push(IncomingMessage {
                    from: msg.sender,
                    text,
                    received_at: std::time::SystemTime::now(),
                });
                let entry = self
                    .pending_bursts
                    .entry(msg.sender)
                    .or_insert((0, Instant::now()));
                entry.0 += 1;
                entry.1 = Instant::now();
            }
            TYPE_SYNC => {
                let Ok(sync) = frame.decode::<SyncPayload>() else {
                    return;
                };
                let Some(group_id) = self.peers.get(conn).map(|p| p.group_id) else {
                    return;
                };
                if let Some(max_lamport) = sync.max_lamport {
                    // Request: reply with everything newer.
                    let messages = self
                        .groups
                        .get(&group_id)
                        .map(|g| g.since(max_lamport))
                        .unwrap_or_default();
                    let reply = match Frame::typed(
                        TYPE_SYNC,
                        &SyncPayload {
                            max_lamport: None,
                            messages: Some(messages),
                        },
                    ) {
                        Ok(f) => f,
                        Err(_) => return,
                    };
                    self.send_to(conn, &reply).await;
                } else if let Some(messages) = sync.messages {
                    // Reply: merge into local history.
                    let merged = match self.groups.get_mut(&group_id) {
                        Some(group) => group.merge(&messages),
                        None => 0,
                    };
                    if merged > 0 {
                        tracing::info!(
                            "Synced {merged} message(s) from {}",
                            self.contact_name(conn)
                        );
                    }
                }
            }
            TYPE_DECOY => {
                let Ok(decoy) = frame.decode::<DecoyPayload>() else {
                    return;
                };
                if let Some(alert) = self.s7.on_bundle(conn, &decoy.destination) {
                    self.push_alert(alert);
                }
            }
            TYPE_DECOY_PUBLISH => {
                let Ok(published) = frame.decode::<DecoyPublishPayload>() else {
                    return;
                };
                if let Some(peer) = self.peers.get_mut(conn) {
                    peer.decoys = published.decoys;
                }
            }
            other => {
                tracing::warn!(
                    "unexpected frame type {other:#04x} from {}",
                    crate::sensors::short_key(conn)
                );
            }
        }
    }

    /// Опубликовать свои активные decoy-EID пиру (S7). В Legacy decoys едут
    /// внутри handshake-сообщений; в ratchet-режиме — фреймом 0x08 сразу
    /// после регистрации пира, с обеих сторон.
    async fn publish_decoys_to(&mut self, peer: &PubKey) {
        let decoys = self.s7.active_decoys().to_vec();
        if let Ok(frame) = Frame::typed(TYPE_DECOY_PUBLISH, &DecoyPublishPayload { decoys }) {
            self.send_to(peer, &frame).await;
        }
    }

    async fn send_to(&mut self, key: &PubKey, frame: &Frame) {
        if let Some(peer) = self.peers.get_mut(key) {
            if let Err(e) = framing::write_frame(&mut peer.writer, frame).await {
                tracing::warn!("send to {} failed: {e}", self.contact_name(key));
            }
        }
    }

    /// Этап A: отправка ratchet-фрейма пиру. Пока от пира не получено ни
    /// одного входящего фрейма, СНАЧАЛА подмешивается активация (пустой
    /// ratchet-фрейм) — идемпотентное восстановление при потере первой
    /// активации в DTN (MUSH-A-SPEC §4.0.1). Если наша сторона ещё не
    /// может шифровать (responder без входящих), фрейм пропускается.
    async fn send_ratchet_to(&mut self, key: &PubKey, frame: Frame) {
        let activation = {
            let Some(peer) = self.peers.get_mut(key) else {
                self.send_to(key, &frame).await;
                return;
            };
            if peer.seen_incoming {
                None
            } else {
                peer.ratchet
                    .as_mut()
                    .and_then(|s| s.encrypt(b""))
                    .and_then(|msg| {
                        Frame::typed(
                            TYPE_RATCHET,
                            &RatchetFrame {
                                id: "activation".to_string(),
                                lamport: 0,
                                message: msg,
                            },
                        )
                        .ok()
                    })
            }
        };
        if let Some(act) = activation {
            self.send_to(key, &act).await;
        }
        self.send_to(key, &frame).await;
    }

    /// Этап A: ретрансляция ratchet-сообщения — НЕ сырого фрейма, а
    /// перешифровка текстом для каждого пира группы (pairwise сессии).
    /// id/lamport сохраняются — CRDT-мерж и дедуп работают как в legacy.
    async fn relay_ratchet(
        &mut self,
        exclude: &PubKey,
        group_id: GroupId,
        id: &str,
        lamport: u64,
        text: &str,
    ) {
        let mut frames: Vec<(PubKey, Frame)> = Vec::new();
        for (key, peer) in self.peers.iter_mut() {
            if key == exclude || peer.group_id != group_id {
                continue;
            }
            let Some(session) = peer.ratchet.as_mut() else {
                continue;
            };
            let Some(encrypted) = session.encrypt(text.as_bytes()) else {
                continue;
            };
            let rf = RatchetFrame {
                id: id.to_string(),
                lamport,
                message: encrypted,
            };
            if let Ok(frame) = Frame::typed(TYPE_RATCHET, &rf) {
                frames.push((*key, frame));
            }
        }
        for (key, frame) in frames {
            self.send_ratchet_to(&key, frame).await;
        }
    }

    /// Forward a frame to every confirmed peer except `exclude` (dedup already done).
    async fn relay(&mut self, exclude: &PubKey, frame: &Frame) {
        let targets: Vec<PubKey> = self
            .peers
            .keys()
            .filter(|k| *k != exclude)
            .copied()
            .collect();
        for target in targets {
            self.send_to(&target, frame).await;
        }
    }

    /// Broadcast a locally produced message to all peers of its group.
    async fn broadcast(&mut self, group_id: &GroupId, frame: &Frame) {
        let targets: Vec<PubKey> = self
            .peers
            .iter()
            .filter(|(_, p)| &p.group_id == group_id)
            .map(|(k, _)| *k)
            .collect();
        for target in targets {
            self.send_to(&target, frame).await;
        }
    }

    /// Flush accumulated per-sender message counts into S1.
    pub fn flush_pending_bursts(&mut self) {
        let now = Instant::now();
        let senders: Vec<PubKey> = self
            .pending_bursts
            .iter()
            .filter(|(_, (_, last))| now.duration_since(*last) >= self.idle_flush)
            .map(|(k, _)| *k)
            .collect();
        for sender in senders {
            if let Some((count, _)) = self.pending_bursts.remove(&sender) {
                if let Some(alert) = self.s1.on_burst(&sender, count) {
                    self.push_alert(alert);
                }
            }
        }
    }

    /// Append a message locally without broadcasting (local echo + sync source).
    pub fn append_local_only(&mut self, group_prefix: &str, text: &str) -> Result<String, String> {
        let group_id = self
            .resolve_group(group_prefix)
            .ok_or_else(|| format!("unknown group '{group_prefix}'"))?;
        let me = self.pubkey();
        let msg = self
            .groups
            .get_mut(&group_id)
            .ok_or("group missing")?
            .append_local(&me, text);
        Ok(msg.id)
    }

    pub async fn send_text(&mut self, group_prefix: &str, text: &str) -> Result<(), String> {
        let group_id = self
            .resolve_group(group_prefix)
            .ok_or_else(|| format!("unknown group '{group_prefix}'"))?;
        let me = self.pubkey();
        let msg = self
            .groups
            .get_mut(&group_id)
            .ok_or("group missing")?
            .append_local(&me, text);
        if self.crypto_mode == CryptoMode::Ratchet {
            // Этап A: текст едет внутри pairwise ratchet AEAD; открытые
            // id/lamport CRDT сохраняют сходимость истории.
            let mut frames: Vec<(PubKey, Frame)> = Vec::new();
            for (key, peer) in self.peers.iter_mut() {
                if peer.group_id != group_id {
                    continue;
                }
                let Some(session) = peer.ratchet.as_mut() else {
                    continue;
                };
                let Some(encrypted) = session.encrypt(text.as_bytes()) else {
                    continue;
                };
                let rf = RatchetFrame {
                    id: msg.id.clone(),
                    lamport: msg.timestamp,
                    message: encrypted,
                };
                if let Ok(frame) = Frame::typed(TYPE_RATCHET, &rf) {
                    frames.push((*key, frame));
                }
            }
            for (key, frame) in frames {
                self.send_ratchet_to(&key, frame).await;
            }
        } else {
            let frame = Frame::typed(TYPE_TEXT, &msg).map_err(|e| e.to_string())?;
            self.broadcast(&group_id, &frame).await;
        }
        println!("[{}] {}: {text} ✓", Self::group_label(&group_id), self.name);
        Ok(())
    }

    pub async fn flood(&mut self, group_prefix: &str, n: usize) -> Result<(), String> {
        let group_id = self
            .resolve_group(group_prefix)
            .ok_or_else(|| format!("unknown group '{group_prefix}'"))?;
        let me = self.pubkey();
        let mut frame_buf = Vec::new();
        for i in 0..n {
            let msg = self
                .groups
                .get_mut(&group_id)
                .ok_or("group missing")?
                .append_local(&me, &format!("flood {i}"));
            frame_buf.push(Frame::typed(TYPE_TEXT, &msg).map_err(|e| e.to_string())?);
        }
        for frame in &frame_buf {
            self.broadcast(&group_id, frame).await;
        }
        println!(
            "[INFO] Sent {n} messages to {}",
            Self::group_label(&group_id)
        );
        // Self-check: flooding is exactly the burst pattern S1 watches for.
        if let Some(alert) = self.s1.on_burst(&me, n) {
            self.push_alert(alert);
        }
        Ok(())
    }

    pub async fn request_sync(&mut self, group_prefix: &str) -> Result<(), String> {
        let group_id = self
            .resolve_group(group_prefix)
            .ok_or_else(|| format!("unknown group '{group_prefix}'"))?;
        let max_lamport = self
            .groups
            .get(&group_id)
            .map(|g| g.max_lamport())
            .unwrap_or(0);
        let frame = Frame::typed(
            TYPE_SYNC,
            &SyncPayload {
                max_lamport: Some(max_lamport),
                messages: None,
            },
        )
        .map_err(|e| e.to_string())?;
        let targets: Vec<PubKey> = self
            .peers
            .iter()
            .filter(|(_, p)| p.group_id == group_id)
            .map(|(k, _)| *k)
            .collect();
        for target in targets {
            self.send_to(&target, &frame).await;
        }
        Ok(())
    }

    pub async fn send_decoy(&mut self) -> Result<(), String> {
        // Pick the first peer and one of ITS published decoys (honeypot bait).
        let target = *self.peers.keys().next().ok_or("no peers connected")?;
        let decoy = self
            .peers
            .get(&target)
            .and_then(|p| p.decoys.first().copied())
            .ok_or("peer published no decoys")?;
        let frame = Frame::typed(TYPE_DECOY, &DecoyPayload { destination: decoy })
            .map_err(|e| e.to_string())?;
        self.send_to(&target, &frame).await;
        println!("[INFO] Sent bundle to decoy EID: {}", hex(&decoy));
        Ok(())
    }

    /// Execute one CLI command (CLI-SPEC §2.3). Returns true if the node should quit.
    pub async fn handle_command(&mut self, line: &str) -> bool {
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else {
            return false;
        };
        let rest: Vec<&str> = parts.collect();
        match cmd {
            "/send" => {
                let group = rest.first().copied().unwrap_or("default");
                let text = rest[1..].join(" ");
                if text.is_empty() {
                    println!("usage: /send <group> <text>");
                } else if let Err(e) = self.send_text(group, &text).await {
                    println!("[ERROR] {e}");
                }
            }
            "/join" => match rest.first() {
                Some(invite) => {
                    println!("[INFO] Connecting via invite...");
                    match self.join_peer(invite).await {
                        Ok(()) => println!("[INFO] Joined."),
                        Err(e) => println!("[ERROR] {e}"),
                    }
                }
                None => println!("usage: /join <invite-link>"),
            },
            "/invite" => println!("{}", self.invite_now()),
            "/contacts" => {
                if self.contacts.is_empty() {
                    println!("(no contacts)");
                }
                for (key, c) in &self.contacts {
                    let seen = c
                        .last_seen
                        .map(|t| format!("{}s ago", t.elapsed().as_secs()))
                        .unwrap_or_else(|| "never".into());
                    println!(
                        "{} {} (key {}, last seen {})",
                        c.name,
                        crate::sensors::short_key(key),
                        &hex(key)[..12],
                        seen
                    );
                }
            }
            "/sync" => {
                let group = rest.first().copied().unwrap_or("default");
                if let Err(e) = self.request_sync(group).await {
                    println!("[ERROR] {e}");
                } else {
                    println!("[INFO] Sync requested.");
                }
            }
            "/flood" => {
                let n: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(50);
                let group = rest.get(1).copied().unwrap_or("default");
                if let Err(e) = self.flood(group, n).await {
                    println!("[ERROR] {e}");
                }
            }
            "/decoy" => {
                if let Err(e) = self.send_decoy().await {
                    println!("[ERROR] {e}");
                }
            }
            "/alert" => {
                self.flush_pending_bursts();
                if self.alerts.is_empty() {
                    println!("(no alerts)");
                }
                for (at, alert) in &self.alerts {
                    println!("[{at}] {}", alert.render());
                }
            }
            "/help" => {
                println!("Commands: /send <group> <text> | /join <invite> | /invite | /contacts | /sync <group> | /flood <n> [group] | /decoy | /alert | /quit");
            }
            "/quit" => return true,
            other => println!("unknown command '{other}' (try /help)"),
        }
        false
    }

    // ---- test/debug accessors ----

    pub fn alert_strings(&self) -> Vec<String> {
        self.alerts.iter().map(|(_, a)| a.render()).collect()
    }

    pub fn history_texts(&self, group_prefix: &str) -> Vec<String> {
        let Some(group_id) = self.resolve_group(group_prefix) else {
            return Vec::new();
        };
        match self.groups.get(&group_id) {
            // Этап A: в ratchet-режиме у принимающей стороны content — plaintext
            // (текст шёл внутри ratchet AEAD), у отправителя — group_key AES.
            // Порядок: сначала AES-дешифровка, fallback — UTF-8 plaintext.
            Some(group) => group
                .history()
                .iter()
                .map(|m| {
                    group
                        .decrypt(m)
                        .unwrap_or_else(|| String::from_utf8_lossy(&m.content).into_owned())
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// The node's working group (created lazily by `invite_now`).
    pub fn default_group_id(&self) -> Option<GroupId> {
        self.groups.keys().next().copied()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    // ---- connection loops ----

    /// Drive one accepted socket: handshake first, then frame dispatch.
    pub async fn handle_connection(node: Arc<Mutex<Node>>, socket: TcpStream) {
        let (mut reader, writer) = socket.into_split();
        let Ok(Some(first)) = framing::read_frame(&mut reader).await else {
            // Молчаливая смерть handler'а — главная невидимая дыра
            // hardware-прогона (таймаут на recv выглядит как «нода глухая»).
            tracing::warn!("connection handler: first frame read failed");
            return;
        };

        // Этап A, ratchet-режим: первый фрейм — X3dhInit (SNS не используется).
        if first.msg_type == TYPE_X3DH_INIT {
            let Ok(init) = first.decode::<ratchet::X3dhInit>() else {
                tracing::warn!("connection handler: x3dh init decode failed");
                return;
            };
            let completed = {
                let mut n = node.lock().await;
                n.accept_ratchet(&init)
            };
            let (start, first_msg) = match completed {
                Some(v) => v,
                None => {
                    tracing::warn!("x3dh init rejected");
                    return;
                }
            };
            let peer_key = init.identity;
            let peer_name = String::from_utf8_lossy(&first_msg).into_owned();
            {
                let mut n = node.lock().await;
                let group_id = n.ensure_default_group();
                n.contacts.insert(
                    peer_key,
                    Contact {
                        name: peer_name.clone(),
                        last_seen: Some(Instant::now()),
                    },
                );
                if let Some(group) = n.groups.get_mut(&group_id) {
                    group.add_member(peer_key);
                }
                let spk = n.pre_key_store.signed_pre_key_secret().clone();
                n.peers.insert(
                    peer_key,
                    Peer {
                        writer,
                        name: peer_name.clone(),
                        decoys: Vec::new(),
                        session_key: [0u8; 32],
                        group_id,
                        ratchet: Some(ratchet::RatchetSession::new_responder(&start, spk)),
                        seen_incoming: false,
                    },
                );
                n.emit(NodeEvent::HandshakeComplete {
                    peer: peer_key,
                    name: n
                        .peers
                        .get(&peer_key)
                        .map(|p| p.name.clone())
                        .unwrap_or_default(),
                });
                // Parity с Legacy `register_peer`: handshake-completion виден
                // в логах (smoke, Logcat) на обеих сторонах.
                tracing::info!(
                    "Handshake complete with {} ({}). Session key established.",
                    peer_name,
                    crate::sensors::short_key(&peer_key),
                );
            }
            // S7: отдаём initiator'у свои decoy-EID (в Legacy это поле
            // handshake-reply; здесь — фрейм 0x08).
            {
                let mut n = node.lock().await;
                n.publish_decoys_to(&peer_key).await;
            }
            loop {
                match framing::read_frame(&mut reader).await {
                    Ok(Some(frame)) => {
                        let mut n = node.lock().await;
                        n.process_frame(&peer_key, &frame).await;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("connection error: {e}");
                        break;
                    }
                }
            }
            let mut n = node.lock().await;
            n.peers.remove(&peer_key);
            n.contacts
                .entry(peer_key)
                .and_modify(|c| c.last_seen = Some(Instant::now()));
            return;
        }

        if first.msg_type != TYPE_HANDSHAKE {
            tracing::warn!("connection without handshake dropped");
            return;
        }
        let Ok(offer) = first.decode::<HandshakeMessage>() else {
            return;
        };
        let accepted = {
            let mut n = node.lock().await;
            n.accept_handshake(&offer)
        };
        let (reply, session) = match accepted {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("handshake rejected: {e}");
                return;
            }
        };
        let Ok(reply_frame) = Frame::typed(TYPE_HANDSHAKE, &reply) else {
            return;
        };
        let mut writer = writer;
        if framing::write_frame(&mut writer, &reply_frame)
            .await
            .is_err()
        {
            return;
        }
        let peer_key = session.peer_pubkey;
        {
            let mut n = node.lock().await;
            // Inbound peer joins our working group (the one our invites carry).
            let group_id = n.ensure_default_group();
            n.register_peer(session, writer, offer.decoys.clone(), group_id);
        }
        loop {
            match framing::read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    let mut n = node.lock().await;
                    n.process_frame(&peer_key, &frame).await;
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("connection error: {e}");
                    break;
                }
            }
        }
        let mut n = node.lock().await;
        n.peers.remove(&peer_key);
        n.contacts
            .entry(peer_key)
            .and_modify(|c| c.last_seen = Some(Instant::now()));
    }

    /// Read loop for an outbound (joined) connection.
    pub async fn outbound_loop(
        node: Arc<Mutex<Node>>,
        peer_key: PubKey,
        mut reader: OwnedReadHalf,
    ) {
        loop {
            match framing::read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    let mut n = node.lock().await;
                    n.process_frame(&peer_key, &frame).await;
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("peer connection error: {e}");
                    break;
                }
            }
        }
        let mut n = node.lock().await;
        n.peers.remove(&peer_key);
    }
}

/// Run a node: bind, print banner + invite, accept loop, command loop, flush tick.
pub async fn run(
    node: Arc<Mutex<Node>>,
    mut commands: mpsc::UnboundedReceiver<String>,
    invite_tx: Option<oneshot::Sender<String>>,
) -> io::Result<()> {
    let configured = node.lock().await.configured_port;
    let listener = TcpListener::bind(("127.0.0.1", configured)).await?;
    let bound = listener.local_addr()?;
    {
        let mut n = node.lock().await;
        n.set_bound(bound);
        n.set_self_ref(node.clone());
    }

    let (name, identity_url, invite) = {
        let mut n = node.lock().await;
        let invite = n.invite_now();
        (
            n.name.clone(),
            format!("mush://{}", base64::encode(&n.pubkey())),
            invite,
        )
    };
    let group_label = node
        .lock()
        .await
        .default_group_id()
        .map(|id| Node::group_label(&id))
        .unwrap_or_else(|| "none (create via /invite)".to_string());
    println!("[INFO] Node started: {bound}");
    println!("[INFO] Identity: {identity_url}");
    println!("[INFO] Invite link: {invite}");
    println!("[INFO] Default group: {group_label} (use in /send, /sync)");
    if let Some(tx) = invite_tx {
        let _ = tx.send(invite);
    }

    // Accept loop.
    {
        let node = node.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((socket, _)) => {
                        let node = node.clone();
                        tokio::spawn(async move {
                            Node::handle_connection(node, socket).await;
                        });
                    }
                    Err(e) => {
                        tracing::error!("accept failed: {e}");
                        break;
                    }
                }
            }
        });
    }

    // S1 flush tick.
    {
        let node = node.clone();
        tokio::spawn(async move {
            loop {
                let every = node.lock().await.flush_every;
                tokio::time::sleep(every).await;
                node.lock().await.flush_pending_bursts();
            }
        });
    }

    println!("[INFO] Ready, {name}. Type /help for commands.");
    while let Some(line) = commands.recv().await {
        let quit = node.lock().await.handle_command(&line).await;
        if quit {
            break;
        }
    }
    println!("[INFO] Bye.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A'-era helper: явный Legacy-режим. Эти тесты — регрессия Legacy до
    /// его удаления в этапе C (MUSH-B-SPEC §0); дефолт узла с этапа B —
    /// Ratchet (`test_node_default` / `start_node_mode(.., Ratchet)`).
    pub(crate) fn test_node(name: &str) -> Arc<Mutex<Node>> {
        let mut node = Node::new(name, 0);
        node.set_crypto_mode(CryptoMode::Legacy);
        node.flush_every = Duration::from_millis(50);
        node.idle_flush = Duration::from_millis(100);
        Arc::new(Mutex::new(node))
    }

    pub(crate) fn test_node_mode(name: &str, mode: CryptoMode) -> Arc<Mutex<Node>> {
        let mut node = Node::new(name, 0);
        node.flush_every = Duration::from_millis(50);
        node.idle_flush = Duration::from_millis(100);
        node.set_crypto_mode(mode);
        Arc::new(Mutex::new(node))
    }

    pub(crate) async fn start_node(name: &str) -> (Arc<Mutex<Node>>, String) {
        let node = test_node(name);
        let (tx, rx) = oneshot::channel();
        let n2 = node.clone();
        tokio::spawn(async move {
            let _ = run(n2, mpsc::unbounded_channel().1, Some(tx)).await;
        });
        let invite = rx.await.expect("invite");
        (node, invite)
    }

    pub(crate) async fn start_node_mode(
        name: &str,
        mode: CryptoMode,
    ) -> (Arc<Mutex<Node>>, String) {
        let node = test_node_mode(name, mode);
        let (tx, rx) = oneshot::channel();
        let n2 = node.clone();
        tokio::spawn(async move {
            let _ = run(n2, mpsc::unbounded_channel().1, Some(tx)).await;
        });
        let invite = rx.await.expect("invite");
        (node, invite)
    }

    /// Инвариант этапа B (MUSH-B-SPEC §0): дефолт Node::new — Ratchet,
    /// узел сразу готов к X3DH (OPK-очередь пополнена).
    #[test]
    fn default_crypto_mode_is_ratchet() {
        let node = Node::new("default", 0);
        assert_eq!(node.crypto_mode(), CryptoMode::Ratchet);
        assert!(node.pre_key_store.pending_count() >= 50);
    }

    pub(crate) async fn cmd(node: &Arc<Mutex<Node>>, line: &str) {
        node.lock().await.handle_command(line).await;
    }

    pub(crate) async fn wait_for<F, Fut>(mut cond: F, timeout_ms: u64, what: &str)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while Instant::now() < deadline {
            if cond().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("condition not met within {timeout_ms}ms: {what}");
    }

    #[tokio::test]
    async fn invite_parse_roundtrip() {
        let mut node = Node::new("alice", 7878);
        node.set_bound("127.0.0.1:7878".parse().unwrap());
        let s = node.invite_now();
        let invite = Invite::parse(&s).expect("parse");
        assert_eq!(invite.pubkey, node.pubkey());
        assert_eq!(invite.addr, "127.0.0.1:7878");
        assert_eq!(
            invite.group_id,
            node.default_group_id().expect("group created by invite")
        );
    }

    #[tokio::test]
    async fn invite_uses_advertised_addr_override() {
        let mut node = Node::new("alice", 7878);
        node.set_bound("127.0.0.1:7878".parse().unwrap());
        let s = node.invite_now();
        assert!(s.contains("addr=127.0.0.1:7878"));

        node.set_advertised_addr(Some("192.168.43.1:7878".to_string()));
        let s = node.invite_now();
        assert!(s.contains("addr=192.168.43.1:7878"));

        node.set_advertised_addr(None);
        let s = node.invite_now();
        assert!(s.contains("addr=127.0.0.1:7878"));
    }

    #[tokio::test]
    async fn two_nodes_handshake_and_exchange() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;

        cmd(&b, &format!("/join {invite}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 },
            3000,
            "a+b peer 1",
        )
        .await;

        cmd(&b, "/send default hello alice").await;
        wait_for(
            || async { a.lock().await.history_texts("default").len() == 1 },
            3000,
            "a history hello",
        )
        .await;
        assert_eq!(
            a.lock().await.history_texts("default"),
            vec!["hello alice".to_string()]
        );
    }

    #[tokio::test]
    async fn invite_is_one_time_token() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        let (c, _inv_c) = start_node("carol").await;

        cmd(&b, &format!("/join {invite}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;

        // Same invite again: nonce already consumed -> rejected.
        cmd(&c, &format!("/join {invite}")).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(c.lock().await.peer_count(), 0);
        assert_eq!(a.lock().await.peer_count(), 1);
    }

    #[tokio::test]
    async fn three_nodes_converge_via_relay_and_sync() {
        let (a, invite_a) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        let (c, _inv_c) = start_node("carol").await;

        cmd(&b, &format!("/join {invite_a}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;
        // Fresh one-time invite for carol.
        let invite_a2 = a.lock().await.invite_now();
        cmd(&c, &format!("/join {invite_a2}")).await;
        wait_for(
            || async { a.lock().await.peer_count() == 2 },
            3000,
            "a peer 2",
        )
        .await;

        // Bob sends: relay alice -> carol must deliver it.
        cmd(&b, "/send default from bob").await;
        wait_for(
            || async { c.lock().await.history_texts("default").len() == 1 },
            3000,
            "c local secret",
        )
        .await;

        // Alice appends locally WITHOUT broadcast; carol and bob pull it via /sync.
        a.lock()
            .await
            .append_local_only("default", "local secret")
            .unwrap();
        cmd(&c, "/sync default").await;
        cmd(&b, "/sync default").await;
        wait_for(
            || async {
                c.lock()
                    .await
                    .history_texts("default")
                    .contains(&"local secret".to_string())
            },
            3000,
            "histories converge",
        )
        .await;
        wait_for(
            || async {
                b.lock()
                    .await
                    .history_texts("default")
                    .contains(&"local secret".to_string())
            },
            3000,
            "len 2",
        )
        .await;

        // Identical history everywhere (criterion 3).
        let ha = a.lock().await.history_texts("default");
        let hb = b.lock().await.history_texts("default");
        let hc = c.lock().await.history_texts("default");
        assert_eq!(ha, hb);
        assert_eq!(hb, hc);
        assert_eq!(ha.len(), 2);
    }

    #[tokio::test]
    async fn s1_detects_flood_on_receiver() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        cmd(&b, &format!("/join {invite}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;

        cmd(&b, "/flood 50 default").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .alert_strings()
                    .iter()
                    .any(|s| s.contains("S1 Alert"))
            },
            5000,
            "a s1 alert",
        )
        .await;
        // Sender-side self check also fires.
        assert!(b
            .lock()
            .await
            .alert_strings()
            .iter()
            .any(|s| s.contains("S1 Alert")));
    }

    #[tokio::test]
    async fn s7_detects_decoy_hit() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        cmd(&b, &format!("/join {invite}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;

        cmd(&b, "/decoy").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .alert_strings()
                    .iter()
                    .any(|s| s.contains("S7 Alert"))
            },
            3000,
            "s1 alert",
        )
        .await;
    }

    #[tokio::test]
    async fn backpressure_caps_inbox() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        cmd(&b, &format!("/join {invite}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 },
            3000,
            "a peer 1",
        )
        .await;

        cmd(&b, "/flood 150 default").await;
        wait_for(
            || async { a.lock().await.inbox.len() >= 100 },
            5000,
            "inbox 100",
        )
        .await;
        let inbox = &a.lock().await.inbox;
        assert!(inbox.len() <= 100);
        assert!(inbox.messages().back().unwrap().text.contains("flood 149"));
        assert!(!inbox.messages().front().unwrap().text.contains("flood 0"));
    }
}

#[cfg(test)]
mod ratchet_mode_tests {
    //! Этап A интеграция: X3DH + Double Ratchet поверх существующего
    //! транспорта (MUSH-A-SPEC §5). Legacy-режим не тронут — dual-mode CI.
    use super::tests::*;
    use super::*;

    /// S7 в ratchet-режиме: decoy-EID публикуются фреймом 0x08 (в Legacy —
    /// полем handshake), /decoy противника взводит алёрт на владельце EID.
    #[tokio::test]
    async fn ratchet_mode_decoy_publish_and_s7_alert() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        let (b, _invite_b) = start_node_mode("bob", CryptoMode::Ratchet).await;
        cmd(&b, &format!("/join {invite_a}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 },
            3000,
            "peers connected",
        )
        .await;
        // Decoy-обмен состоялся в обе стороны.
        wait_for(
            || async {
                !b.lock()
                    .await
                    .peers
                    .values()
                    .next()
                    .unwrap()
                    .decoys
                    .is_empty()
                    && !a
                        .lock()
                        .await
                        .peers
                        .values()
                        .next()
                        .unwrap()
                        .decoys
                        .is_empty()
            },
            3000,
            "decoy publish both ways",
        )
        .await;

        cmd(&b, "/decoy").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .alert_strings()
                    .iter()
                    .any(|s| s.contains("S7 Alert"))
            },
            3000,
            "s7 alert on a",
        )
        .await;
    }

    #[tokio::test]
    async fn ratchet_mode_two_nodes_exchange() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        let (b, _invite_b) = start_node_mode("bob", CryptoMode::Ratchet).await;
        assert!(invite_a.contains("pk="), "invite несёт pk= bundle");

        cmd(&b, &format!("/join {invite_a}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;

        cmd(&b, "/send default hello over ratchet").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .history_texts("default")
                    .contains(&"hello over ratchet".to_string())
            },
            3000,
            "a got hello ratchet",
        )
        .await;

        cmd(&a, "/send default reply over ratchet").await;
        wait_for(
            || async {
                b.lock()
                    .await
                    .history_texts("default")
                    .contains(&"reply over ratchet".to_string())
            },
            3000,
            "b got reply",
        )
        .await;
    }

    #[tokio::test]
    async fn ratchet_mode_four_nodes_converge_via_relay_and_sync() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        let (b, _ib) = start_node_mode("bob", CryptoMode::Ratchet).await;
        let (c, _ic) = start_node_mode("carol", CryptoMode::Ratchet).await;
        let (d, _id) = start_node_mode("dave", CryptoMode::Ratchet).await;

        cmd(&b, &format!("/join {invite_a}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;
        let invite_a2 = a.lock().await.invite_now();
        cmd(&c, &format!("/join {invite_a2}")).await;
        wait_for(
            || async { a.lock().await.peer_count() == 2 },
            3000,
            "a peer 2",
        )
        .await;
        let invite_a3 = a.lock().await.invite_now();
        cmd(&d, &format!("/join {invite_a3}")).await;
        wait_for(
            || async { a.lock().await.peer_count() == 3 },
            3000,
            "a peer 3",
        )
        .await;

        // Bob -> все (relay через alice).
        cmd(&b, "/send default from bob").await;
        wait_for(
            || async {
                d.lock()
                    .await
                    .history_texts("default")
                    .contains(&"from bob".to_string())
            },
            3000,
            "d got from bob",
        )
        .await;

        // Alice appends локально БЕЗ broadcast; carol и dave догоняют /sync.
        a.lock()
            .await
            .append_local_only("default", "local secret")
            .unwrap();
        cmd(&c, "/sync default").await;
        cmd(&d, "/sync default").await;
        cmd(&b, "/sync default").await;
        wait_for(
            || async {
                d.lock()
                    .await
                    .history_texts("default")
                    .contains(&"local secret".to_string())
            },
            3000,
            "d got local secret",
        )
        .await;

        // Истории сходятся на всех четырёх.
        let ha = a.lock().await.history_texts("default");
        let hb = b.lock().await.history_texts("default");
        let hc = c.lock().await.history_texts("default");
        let hd = d.lock().await.history_texts("default");
        assert_eq!(ha, hb);
        assert_eq!(hb, hc);
        assert_eq!(hc, hd);
        assert_eq!(ha.len(), 2);
    }

    #[tokio::test]
    async fn ratchet_mode_session_export_import_continues() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        let (b, _ib) = start_node_mode("bob", CryptoMode::Ratchet).await;
        cmd(&b, &format!("/join {invite_a}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;

        cmd(&b, "/send default before persist").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .history_texts("default")
                    .contains(&"before persist".to_string())
            },
            3000,
            "a got before persist",
        )
        .await;

        // Persist bob'овой сессии с alice, импорт обратно (roundtrip API).
        let peer_key_a = a.lock().await.pubkey();
        let state = b
            .lock()
            .await
            .export_peer_session(&peer_key_a)
            .expect("session exported");
        assert!(b.lock().await.import_peer_session(&peer_key_a, &state));

        // Обмен продолжается в обе стороны после restore.
        cmd(&b, "/send default after persist").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .history_texts("default")
                    .contains(&"after persist".to_string())
            },
            3000,
            "a got after persist",
        )
        .await;
        cmd(&a, "/send default back to bob").await;
        wait_for(
            || async {
                b.lock()
                    .await
                    .history_texts("default")
                    .contains(&"back to bob".to_string())
            },
            3000,
            "b got back to bob",
        )
        .await;
    }

    #[tokio::test]
    async fn node_state_export_import_restores_identity_contacts_and_sessions() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        let (b, _ib) = start_node_mode("bob", CryptoMode::Ratchet).await;
        cmd(&b, &format!("/join {invite_a}")).await;
        wait_for(
            || async { b.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 },
            3000,
            "b peer 1",
        )
        .await;
        cmd(&b, "/send default hello for export").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .history_texts("default")
                    .contains(&"hello for export".to_string())
            },
            3000,
            "a got hello",
        )
        .await;

        let peer_key_a = a.lock().await.pubkey();
        // Контакт на стороне b (alice — собеседник b).
        b.lock().await.contacts.insert(
            peer_key_a,
            Contact {
                name: "alice-from-test".to_string(),
                last_seen: None,
            },
        );

        // Export -> CBOR -> import (полный persist-цикл, MUSH-B-SPEC §3).
        let state = b.lock().await.export_state();
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&state, &mut cbor).expect("cbor");
        // Soft limit 1 MB, НЕ invariant (MUSH-B-SPEC §1.3): сессии ≈ 2–8 KB
        // каждая, при 10+ контактах 64 KB пробивается — 1 MB с запасом.
        // Превышение = повод для streaming export в следующей версии формата.
        assert!(
            cbor.len() < 1024 * 1024,
            "NodeState {} bytes >= 1 MB soft limit",
            cbor.len()
        );
        let decoded: NodeState = ciborium::de::from_reader(cbor.as_slice()).expect("cbor decode");
        let mut restored = Node::import_state(&decoded).expect("import");

        // Identity/pubkey совпадает с исходным узлом.
        assert_eq!(restored.pubkey(), b.lock().await.pubkey());
        assert_eq!(restored.name, "bob");
        assert_eq!(restored.crypto_mode(), CryptoMode::Ratchet);
        // OPK-очередь восстановлена из persist (не дефолтные 50 от new()).
        assert_eq!(
            restored.pre_key_store.pending_count(),
            b.lock().await.pre_key_store.pending_count()
        );

        // Сессии — в restored_sessions с правильными session_id.
        let orig_state = b
            .lock()
            .await
            .export_peer_session(&peer_key_a)
            .expect("orig session");
        let session = restored
            .restored_session(&peer_key_a)
            .expect("restored session");
        assert_eq!(session.export_state().session_id, orig_state.session_id);
        // take одноразовый.
        assert!(restored.take_restored_session(&peer_key_a).is_some());
        assert!(restored.take_restored_session(&peer_key_a).is_none());
        // Контакты на месте.
        assert!(restored.contacts.contains_key(&peer_key_a));
    }

    #[tokio::test]
    async fn node_state_import_rejects_bad_version_and_mode() {
        let node = test_node_mode("solo", CryptoMode::Ratchet);
        let mut state = node.lock().await.export_state();
        state.version = 99;
        assert!(Node::import_state(&state).is_none());
        let mut state = node.lock().await.export_state();
        state.crypto_mode = "quantum".to_string();
        assert!(Node::import_state(&state).is_none());
    }

    #[tokio::test]
    async fn ratchet_mode_invite_roundtrip_tolerates_unknown_fields() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        let parsed = Invite::parse(&invite_a).unwrap();
        assert!(parsed.pk.is_some());
        // Tolerant parsing: неизвестное поле не ломает invite.
        let tampered = format!("{invite_a}&futurefield=xyz123");
        let parsed2 = Invite::parse(&tampered).unwrap();
        assert_eq!(parsed2.pk, parsed.pk);
        assert_eq!(parsed2.group_id, parsed.group_id);
        let _ = a.lock().await.peer_count();
    }
}

#[cfg(test)]
mod activation_recovery_tests {
    //! Потеря активационного фрейма в DTN: обмен обязан восстановиться
    //! через folding активации в исходящие фреймы (MUSH-A-SPEC §4.0.1).
    use super::tests::*;
    use super::{run, CryptoMode};
    use tokio::sync::{mpsc, oneshot};

    #[tokio::test]
    async fn lost_activation_recovers_on_first_contact() {
        let (a, invite_a) = start_node_mode("alice", CryptoMode::Ratchet).await;
        // Bob joins С активацией, но активационный фрейм «теряется в DTN»
        // (эмулЯция: отправка отключена на момент join).
        let bob = {
            let node = test_node_mode("bob", CryptoMode::Ratchet);
            node.lock().await.set_activation_enabled(false);
            node
        };
        let (tx, rx) = oneshot::channel();
        {
            let n2 = bob.clone();
            tokio::spawn(async move {
                let _ = run(n2, mpsc::unbounded_channel().1, Some(tx)).await;
            });
        }
        let _invite_b = rx.await.expect("invite");

        cmd(&bob, &format!("/join {invite_a}")).await;
        wait_for(
            || async { bob.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 },
            3000,
            "peers connected",
        )
        .await;

        // Bob пишет первым: вместе с сообщением уходит и (folded) активация.
        cmd(&bob, "/send default hello after loss").await;
        wait_for(
            || async {
                a.lock()
                    .await
                    .history_texts("default")
                    .contains(&"hello after loss".to_string())
            },
            3000,
            "a got message",
        )
        .await;

        // Теперь alice (responder-сторона, получившая активацию) может
        // ответить — при потерянной навсегда активации этот шаг был бы
        // невозможен (send-цепочка не построена).
        cmd(&a, "/send default recovered reply").await;
        wait_for(
            || async {
                bob.lock()
                    .await
                    .history_texts("default")
                    .contains(&"recovered reply".to_string())
            },
            3000,
            "b got reply (activation recovered)",
        )
        .await;
    }
}
