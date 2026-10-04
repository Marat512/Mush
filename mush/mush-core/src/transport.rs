use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex, oneshot};
use tokio::time::timeout;

use crate::crdt::{hex, AdHocGroup, ChatMessage, GroupId, PubKey};
use crate::framing::{self, Frame, TYPE_DECOY, TYPE_HANDSHAKE, TYPE_SYNC, TYPE_TEXT};
use crate::identity::Identity;
use crate::sensors::{now_millis, Alert, ContactRateSensor, DecoyEidSensor, InboxBuffer, IncomingMessage};
use crate::sns::{HandshakeInitiator, HandshakeMessage, HandshakeResponder, HandshakeError, SnsSession, NONCE_TTL};
use crate::base64;
use rand::RngCore;

/// One confirmed peer connection.
struct Peer {
    writer: OwnedWriteHalf,
    name: String,
    decoys: Vec<[u8; 32]>,
    #[allow(dead_code)] // established per A'-SPEC §3.1; chat payload uses group_key in A'
    session_key: [u8; 32],
    group_id: GroupId,
}

/// Known contact (CLI-SPEC: /contacts).
#[derive(Debug, Clone)]
pub struct Contact {
    pub name: String,
    pub last_seen: Option<Instant>,
}

/// Invite-link (A'-SPEC §7.2, CLI adaptation): carries pubkey, one-time nonce,
/// rendezvous address, and the ad-hoc group material that Android puts in QR.
#[derive(Debug, Clone)]
pub struct Invite {
    pub pubkey: PubKey,
    pub nonce: [u8; 16],
    pub addr: String,
    pub group_id: GroupId,
    pub group_key: [u8; 32],
}

impl Invite {
    pub fn render(&self) -> String {
        format!(
            "mush://{}&nonce={}&addr={}&group={}-{}",
            base64::encode(&self.pubkey),
            base64::encode(&self.nonce),
            self.addr,
            base64::encode(&self.group_id),
            base64::encode(&self.group_key),
        )
    }

    pub fn parse(s: &str) -> Result<Invite, String> {
        let body = s.trim().strip_prefix("mush://").ok_or("not a mush:// link")?;
        // First segment (before any '&') is the bare pubkey base64 — it must be
        // split off BEFORE the key=value parsing because base64 padding is '='.
        let (pk_part, query) = match body.split_once('&') {
            Some((head, rest)) => (head, rest),
            None => (body, ""),
        };
        let pk_bytes = base64::decode(pk_part).ok_or("bad pubkey base64")?;
        let pubkey = <[u8; 32]>::try_from(pk_bytes.as_slice()).map_err(|_| "pubkey must be 32 bytes")?;

        let mut nonce = None;
        let mut addr = None;
        let mut group = None;
        for part in query.split('&') {
            let Some((key, val)) = part.split_once('=') else {
                continue;
            };
            match key {
                "nonce" => {
                    let bytes = base64::decode(val).ok_or("bad nonce base64")?;
                    nonce = Some(<[u8; 16]>::try_from(bytes.as_slice()).map_err(|_| "nonce must be 16 bytes")?);
                }
                "addr" => addr = Some(val.to_string()),
                "group" => {
                    let (gid_b64, gkey_b64) = val.split_once('-').ok_or("bad group part")?;
                    let gid = base64::decode(gid_b64).ok_or("bad group id base64")?;
                    let gkey = base64::decode(gkey_b64).ok_or("bad group key base64")?;
                    group = Some((
                        <[u8; 32]>::try_from(gid.as_slice()).map_err(|_| "group id must be 32 bytes")?,
                        <[u8; 32]>::try_from(gkey.as_slice()).map_err(|_| "group key must be 32 bytes")?,
                    ));
                }
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

/// Decoy frame payload (0x04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoyPayload {
    pub destination: [u8; 32],
}

/// Node state + behavior (CLI-SPEC §2.1). One node = one process.
pub struct Node {
    identity: Identity,
    name: String,
    configured_port: u16,
    bound_addr: Option<String>,
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
}

impl Node {
    pub fn new(name: &str, port: u16) -> Node {
        let identity = Identity::generate();
        let me = identity.pubkey_bytes();
        Node {
            identity,
            name: name.to_string(),
            configured_port: port,
            bound_addr: None,
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

    /// Mint a fresh one-time invite (new nonce, TTL 24h). Creates the node's
    /// default group lazily on first use (creator flow).
    pub fn invite_now(&mut self) -> String {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        self.issued_nonces.insert(nonce, Instant::now());
        let addr = self
            .bound_addr
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{}", self.configured_port));
        let group_id = self.ensure_default_group();
        let group = &self.groups[&group_id];
        Invite {
            pubkey: self.pubkey(),
            nonce,
            addr,
            group_id: group.group_id,
            group_key: group.group_key,
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
        let cutoff = Instant::now().checked_sub(NONCE_TTL).unwrap_or_else(Instant::now);
        self.issued_nonces.retain(|_, at| *at > cutoff);
    }

    fn resolve_group(&self, prefix: &str) -> Option<GroupId> {
        let prefix = prefix.trim_start_matches("group-");
        if prefix.is_empty() || prefix == "default" {
            // The working group: the joined group if there is exactly one with
            // content, otherwise the only group. A node that joined drops its
            // empty auto-created group (see join_peer), keeping this unambiguous.
            return self.groups.keys().next().copied().filter(|_| self.groups.len() == 1);
        }
        let mut found = self
            .groups
            .keys()
            .filter(|&g| hex(g).starts_with(prefix));
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
            Contact { name: session.peer_name.clone(), last_seen: Some(Instant::now()) },
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
            },
        );
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
        let (reply, session) = responder.respond(offer, &self.name, self.s7.active_decoys().to_vec())?;
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

        let initiator = HandshakeInitiator::new(self.identity.clone(), invite.nonce);
        let offer = initiator.start(&self.name, self.s7.active_decoys().to_vec());
        let frame = Frame::typed(TYPE_HANDSHAKE, &offer).map_err(|e| e.to_string())?;
        let mut writer = writer;
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
            self.groups
                .insert(invite.group_id, AdHocGroup::join(invite.group_id, invite.group_key, &me));
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
        self.contacts.entry(*conn).and_modify(|c| c.last_seen = Some(Instant::now()));
        match frame.msg_type {
            TYPE_TEXT => {
                let Ok(msg) = frame.decode::<ChatMessage>() else {
                    tracing::warn!("undecodable text frame from {}", crate::sensors::short_key(conn));
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
                        group.decrypt(&msg).unwrap_or_else(|| "<undecryptable>".into()),
                        Self::group_label(&group_id),
                    ),
                    None => return,
                };
                println!("[{group_label}] {}: {text}", self.contact_name(&msg.sender));
                self.inbox.push(IncomingMessage {
                    from: msg.sender,
                    text,
                    received_at: std::time::SystemTime::now(),
                });
                let entry = self.pending_bursts.entry(msg.sender).or_insert((0, Instant::now()));
                entry.0 += 1;
                entry.1 = Instant::now();
                self.relay(conn, frame).await;
            }
            TYPE_SYNC => {
                let Ok(sync) = frame.decode::<SyncPayload>() else { return };
                let Some(group_id) = self.peers.get(conn).map(|p| p.group_id) else { return };
                if let Some(max_lamport) = sync.max_lamport {
                    // Request: reply with everything newer.
                    let messages = self
                        .groups
                        .get(&group_id)
                        .map(|g| g.since(max_lamport))
                        .unwrap_or_default();
                    let reply = match Frame::typed(TYPE_SYNC, &SyncPayload { max_lamport: None, messages: Some(messages) }) {
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
                        tracing::info!("Synced {merged} message(s) from {}", self.contact_name(conn));
                    }
                }
            }
            TYPE_DECOY => {
                let Ok(decoy) = frame.decode::<DecoyPayload>() else { return };
                if let Some(alert) = self.s7.on_bundle(conn, &decoy.destination) {
                    self.push_alert(alert);
                }
            }
            other => {
                tracing::warn!("unexpected frame type {other:#04x} from {}", crate::sensors::short_key(conn));
            }
        }
    }

    async fn send_to(&mut self, key: &PubKey, frame: &Frame) {
        if let Some(peer) = self.peers.get_mut(key) {
            if let Err(e) = framing::write_frame(&mut peer.writer, frame).await {
                tracing::warn!("send to {} failed: {e}", self.contact_name(key));
            }
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
        let frame = Frame::typed(TYPE_TEXT, &msg).map_err(|e| e.to_string())?;
        self.broadcast(&group_id, &frame).await;
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
        println!("[INFO] Sent {n} messages to {}", Self::group_label(&group_id));
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
        let max_lamport = self.groups.get(&group_id).map(|g| g.max_lamport()).unwrap_or(0);
        let frame = Frame::typed(
            TYPE_SYNC,
            &SyncPayload { max_lamport: Some(max_lamport), messages: None },
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
        let Some(cmd) = parts.next() else { return false };
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
                    println!("{} {} (key {}, last seen {})", c.name, crate::sensors::short_key(key), hex(key)[..12].to_string(), seen);
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
        let Some(group_id) = self.resolve_group(group_prefix) else { return Vec::new() };
        match self.groups.get(&group_id) {
            Some(group) => group
                .history()
                .iter()
                .map(|m| group.decrypt(m).unwrap_or_else(|| "<undecryptable>".into()))
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
            return;
        };
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
        if framing::write_frame(&mut writer, &reply_frame).await.is_err() {
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
        n.contacts.entry(peer_key).and_modify(|c| c.last_seen = Some(Instant::now()));
    }

    /// Read loop for an outbound (joined) connection.
    pub async fn outbound_loop(node: Arc<Mutex<Node>>, peer_key: PubKey, mut reader: OwnedReadHalf) {
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

    fn test_node(name: &str) -> Arc<Mutex<Node>> {
        let mut node = Node::new(name, 0);
        node.flush_every = Duration::from_millis(50);
        node.idle_flush = Duration::from_millis(100);
        Arc::new(Mutex::new(node))
    }

    async fn start_node(name: &str) -> (Arc<Mutex<Node>>, String) {
        let node = test_node(name);
        let (tx, rx) = oneshot::channel();
        let n2 = node.clone();
        tokio::spawn(async move {
            let _ = run(n2, mpsc::unbounded_channel().1, Some(tx)).await;
        });
        let invite = rx.await.expect("invite");
        (node, invite)
    }

    async fn cmd(node: &Arc<Mutex<Node>>, line: &str) {
        node.lock().await.handle_command(line).await;
    }

    async fn wait_for<F, Fut>(mut cond: F, timeout_ms: u64)
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
        panic!("condition not met within {timeout_ms}ms");
    }

    #[tokio::test]
    async fn invite_parse_roundtrip() {
        let mut node = Node::new("alice", 7878);
        node.set_bound("127.0.0.1:7878".parse().unwrap());
        let s = node.invite_now();
        let invite = Invite::parse(&s).expect("parse");
        assert_eq!(invite.pubkey, node.pubkey());
        assert_eq!(invite.addr, "127.0.0.1:7878");
        assert_eq!(invite.group_id, node.default_group_id().expect("group created by invite"));
    }

    #[tokio::test]
    async fn two_nodes_handshake_and_exchange() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;

        cmd(&b, &format!("/join {invite}")).await;
        wait_for(|| async { b.lock().await.peer_count() == 1 && a.lock().await.peer_count() == 1 }, 3000).await;

        cmd(&b, "/send default hello alice").await;
        wait_for(|| async { a.lock().await.history_texts("default").len() == 1 }, 3000).await;
        assert_eq!(a.lock().await.history_texts("default"), vec!["hello alice".to_string()]);
    }

    #[tokio::test]
    async fn invite_is_one_time_token() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        let (c, _inv_c) = start_node("carol").await;

        cmd(&b, &format!("/join {invite}")).await;
        wait_for(|| async { b.lock().await.peer_count() == 1 }, 3000).await;

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
        wait_for(|| async { b.lock().await.peer_count() == 1 }, 3000).await;
        // Fresh one-time invite for carol.
        let invite_a2 = a.lock().await.invite_now();
        cmd(&c, &format!("/join {invite_a2}")).await;
        wait_for(|| async { a.lock().await.peer_count() == 2 }, 3000).await;

        // Bob sends: relay alice -> carol must deliver it.
        cmd(&b, "/send default from bob").await;
        wait_for(|| async { c.lock().await.history_texts("default").len() == 1 }, 3000).await;

        // Alice appends locally WITHOUT broadcast; carol and bob pull it via /sync.
        a.lock().await.append_local_only("default", "local secret").unwrap();
        cmd(&c, "/sync default").await;
        cmd(&b, "/sync default").await;
        wait_for(
            || async { c.lock().await.history_texts("default").contains(&"local secret".to_string()) },
            3000,
        )
        .await;
        wait_for(
            || async { b.lock().await.history_texts("default").contains(&"local secret".to_string()) },
            3000,
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
        wait_for(|| async { b.lock().await.peer_count() == 1 }, 3000).await;

        cmd(&b, "/flood 50 default").await;
        wait_for(
            || async { a.lock().await.alert_strings().iter().any(|s| s.contains("S1 Alert")) },
            5000,
        )
        .await;
        // Sender-side self check also fires.
        assert!(b.lock().await.alert_strings().iter().any(|s| s.contains("S1 Alert")));
    }

    #[tokio::test]
    async fn s7_detects_decoy_hit() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        cmd(&b, &format!("/join {invite}")).await;
        wait_for(|| async { b.lock().await.peer_count() == 1 }, 3000).await;

        cmd(&b, "/decoy").await;
        wait_for(
            || async { a.lock().await.alert_strings().iter().any(|s| s.contains("S7 Alert")) },
            3000,
        )
        .await;
    }

    #[tokio::test]
    async fn backpressure_caps_inbox() {
        let (a, invite) = start_node("alice").await;
        let (b, _inv_b) = start_node("bob").await;
        cmd(&b, &format!("/join {invite}")).await;
        wait_for(|| async { b.lock().await.peer_count() == 1 }, 3000).await;

        cmd(&b, "/flood 150 default").await;
        wait_for(|| async { a.lock().await.inbox.len() >= 100 }, 5000).await;
        let inbox = &a.lock().await.inbox;
        assert!(inbox.len() <= 100);
        assert!(inbox.messages().back().unwrap().text.contains("flood 149"));
        assert!(!inbox.messages().front().unwrap().text.contains("flood 0"));
    }
}
