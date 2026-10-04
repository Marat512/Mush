use std::collections::{BTreeMap, HashSet};

use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::crypto;

pub type PubKey = [u8; 32];
pub type GroupId = [u8; 32];
pub type LamportClock = u64;

/// One chat message (A'-SPEC §4.2). `content` is AES-256-GCM encrypted
/// with the group key; `timestamp` is a Lamport clock, not wall-clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,          // ULID
    pub sender: PubKey,      // Ed25519 pubkey
    pub content: Vec<u8>,    // AES-256-GCM encrypted
    pub timestamp: LamportClock,
    pub nonce: [u8; 12],     // GCM nonce
}

/// Ad-hoc group (A'-SPEC §4.1): no host, CRDT merge on contact.
/// History is stored keyed by (lamport, id) so any merge order yields
/// the identical total order.
#[derive(Debug, Clone)]
pub struct AdHocGroup {
    pub group_id: GroupId,
    pub members: Vec<PubKey>,
    messages: BTreeMap<(LamportClock, String), ChatMessage>,
    seen: HashSet<String>,
    pub group_key: [u8; 32],
    lamport: LamportClock,
}

impl AdHocGroup {
    /// Create a new group: group_id = blake3(creator_pubkey || creation_timestamp).
    pub fn create(creator: &PubKey) -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut hasher = blake3::Hasher::new();
        hasher.update(creator);
        hasher.update(&now_ms.to_le_bytes());
        hasher.update(&key); // uniqueness even for same creator+millisecond
        AdHocGroup {
            group_id: hasher.finalize().into(),
            members: vec![*creator],
            messages: BTreeMap::new(),
            seen: HashSet::new(),
            group_key: key,
            lamport: 0,
        }
    }

    /// Join an existing group from invite material (QR in A', invite string in CLI).
    pub fn join(group_id: GroupId, group_key: [u8; 32], self_key: &PubKey) -> Self {
        AdHocGroup {
            group_id,
            members: vec![*self_key],
            messages: BTreeMap::new(),
            seen: HashSet::new(),
            group_key,
            lamport: 0,
        }
    }

    pub fn group_id_hex(&self) -> String {
        hex(&self.group_id)
    }

    pub fn add_member(&mut self, member: PubKey) {
        if !self.members.contains(&member) {
            self.members.push(member);
        }
    }

    /// Encrypt and locally append a message. Advances the Lamport clock.
    pub fn append_local(&mut self, sender: &PubKey, plaintext: &str) -> ChatMessage {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let content = crypto::aes_gcm_encrypt(&self.group_key, plaintext.as_bytes(), &nonce);
        let msg = ChatMessage {
            id: Ulid::new().to_string(),
            sender: *sender,
            content,
            timestamp: self.next_lamport(),
            nonce,
        };
        self.insert(msg.clone());
        msg
    }

    /// Decrypt a message with the group key.
    pub fn decrypt(&self, msg: &ChatMessage) -> Option<String> {
        crypto::aes_gcm_decrypt(&self.group_key, &msg.content, &msg.nonce)
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }

    /// Insert a message received from the network. Returns true if it was new.
    pub fn insert(&mut self, msg: ChatMessage) -> bool {
        if !self.seen.insert(msg.id.clone()) {
            return false;
        }
        self.lamport = self.lamport.max(msg.timestamp);
        self.messages.insert((msg.timestamp, msg.id.clone()), msg);
        true
    }

    /// CRDT merge: insert remote messages with dedup by MessageID (A'-SPEC §4.2).
    /// Returns the number of messages actually added.
    pub fn merge(&mut self, remote: &[ChatMessage]) -> usize {
        let mut added = 0;
        for msg in remote {
            if self.insert(msg.clone()) {
                added += 1;
            }
        }
        added
    }

    pub fn max_lamport(&self) -> LamportClock {
        self.messages.keys().map(|(l, _)| *l).max().unwrap_or(0)
    }

    /// All messages with clock > `since` (sync protocol, A'-SPEC §4.2).
    pub fn since(&self, since: LamportClock) -> Vec<ChatMessage> {
        self.messages
            .range((since + 1, String::new())..)
            .map(|(_, m)| m.clone())
            .collect()
    }

    pub fn history(&self) -> Vec<ChatMessage> {
        self.messages.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    fn next_lamport(&mut self) -> LamportClock {
        self.lamport += 1;
        self.lamport
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex-decode a string; odd length or invalid chars -> None.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> PubKey {
        [seed; 32]
    }

    #[test]
    fn three_stores_converge_regardless_of_insert_order() {
        let alice = key(1);
        let bob = key(2);
        let carol = key(3);

        let mut store_a = AdHocGroup::create(&alice);
        let gid = store_a.group_id;
        let gkey = store_a.group_key;

        // A produces m1; B (after sync of m1) produces m2; C produces m3 concurrently.
        let m1 = store_a.append_local(&alice, "hello from alice");

        let mut store_b = AdHocGroup::join(gid, gkey, &bob);
        store_b.merge(&store_a.history());
        let m2 = store_b.append_local(&bob, "hi from bob");

        let mut store_c = AdHocGroup::join(gid, gkey, &carol);
        store_c.merge(&store_a.history());
        let m3 = store_c.append_local(&carol, "hey from carol");

        // Merge in different orders.
        store_a.merge(&[m2.clone(), m3.clone()]);
        store_b.merge(&[m3.clone()]);
        store_c.merge(&[m2.clone()]);

        let hist_a: Vec<String> = store_a.history().iter().map(|m| store_a.decrypt(m).unwrap()).collect();
        let hist_b: Vec<String> = store_b.history().iter().map(|m| store_b.decrypt(m).unwrap()).collect();
        let hist_c: Vec<String> = store_c.history().iter().map(|m| store_c.decrypt(m).unwrap()).collect();

        assert_eq!(hist_a, hist_b);
        assert_eq!(hist_b, hist_c);
        assert_eq!(hist_a.len(), 3);
        // Lamport total order: no cycles, every sender sees the same order.
        assert_eq!(hist_a[0], "hello from alice");
    }

    #[test]
    fn merge_is_idempotent() {
        let alice = key(1);
        let bob = key(2);
        let mut a = AdHocGroup::create(&alice);
        let m1 = a.append_local(&alice, "one");
        let m2 = a.append_local(&alice, "two");

        let mut b = AdHocGroup::join(a.group_id, a.group_key, &bob);
        assert_eq!(b.merge(&[m1.clone(), m2.clone()]), 2);
        // Re-merge of the same messages adds nothing (dedup by MessageID).
        assert_eq!(b.merge(&[m1, m2]), 0);
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn sync_returns_only_missing_messages() {
        let alice = key(1);
        let bob = key(2);
        let mut a = AdHocGroup::create(&alice);
        a.append_local(&alice, "one");
        let m2 = a.append_local(&alice, "two");
        let m3 = a.append_local(&alice, "three");

        let peer_max = 1u64; // peer has only lamport 1
        let missing = a.since(peer_max);
        assert_eq!(missing.len(), 2);
        assert_eq!(a.decrypt(&missing[0]).unwrap(), "two");
        assert_eq!(a.max_lamport(), 3);
    }

    #[test]
    fn lamport_advances_past_merged_remote_max() {
        let alice = key(1);
        let bob = key(2);
        let mut a = AdHocGroup::create(&alice);
        let m1 = a.append_local(&alice, "one"); // lamport 1

        let mut b = AdHocGroup::join(a.group_id, a.group_key, &bob);
        b.merge(&[m1]);
        let m2 = b.append_local(&bob, "two");
        assert_eq!(m2.timestamp, 2); // caught up past remote max, no collision

        a.merge(&[m2]);
        let m3 = a.append_local(&alice, "three");
        assert_eq!(m3.timestamp, 3);
        // No duplicate lamport values anywhere => strict total order.
        let stamps: HashSet<u64> = a.history().iter().map(|m| m.timestamp).collect();
        assert_eq!(stamps.len(), a.len());
    }

    #[test]
    fn hundred_messages_three_sources_no_cycles() {
        let alice = key(1);
        let bob = key(2);
        let carol = key(3);
        let mut a = AdHocGroup::create(&alice);
        let gid = a.group_id;
        let gkey = a.group_key;
        let mut b = AdHocGroup::join(gid, gkey, &bob);
        let mut c = AdHocGroup::join(gid, gkey, &carol);

        let mut msgs_a = Vec::new();
        let mut msgs_b = Vec::new();
        let mut msgs_c = Vec::new();
        for _ in 0..100 {
            msgs_a.push(a.append_local(&alice, "a"));
            msgs_b.push(b.append_local(&bob, "b"));
            msgs_c.push(c.append_local(&carol, "c"));
        }
        // Cross-merge everything in chaotic order.
        a.merge(&msgs_b.iter().rev().cloned().collect::<Vec<_>>());
        a.merge(&msgs_c.clone());
        b.merge(&msgs_a.clone());
        b.merge(&msgs_c.clone());
        c.merge(&msgs_a.clone());
        c.merge(&msgs_b.clone());

        let order_a: Vec<String> = a.history().iter().map(|m| m.id.clone()).collect();
        let order_b: Vec<String> = b.history().iter().map(|m| m.id.clone()).collect();
        let order_c: Vec<String> = c.history().iter().map(|m| m.id.clone()).collect();
        assert_eq!(order_a, order_b);
        assert_eq!(order_b, order_c);
        assert_eq!(a.len(), 300);

        // Verify global order is deterministic and acyclic: lamport
        // non-decreasing, (lamport, id) pairs unique (total order via tie-break).
        let stamps: Vec<u64> = a.history().iter().map(|m| m.timestamp).collect();
        for w in stamps.windows(2) {
            assert!(w[0] <= w[1], "lamport order must be monotonic");
        }
        let keys: HashSet<(u64, String)> =
            a.history().iter().map(|m| (m.timestamp, m.id.clone())).collect();
        assert_eq!(keys.len(), a.len(), "(lamport, id) must be unique");
    }

    #[test]
    fn decrypt_rejects_foreign_key() {
        let alice = key(1);
        let mut a = AdHocGroup::create(&alice);
        let m = a.append_local(&alice, "secret");
        let mut foreign = AdHocGroup::create(&alice); // different group key
        foreign.group_id = a.group_id;
        assert!(foreign.decrypt(&m).is_none());
    }

    #[test]
    fn hex_roundtrip() {
        let data = [0xde, 0xad, 0xbe, 0xef, 0x00, 0xff];
        assert_eq!(unhex(&hex(&data)).unwrap(), data);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }
}
