use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::rngs::OsRng;
use rand::RngCore;

use crate::crdt::{hex, PubKey};

/// Evidence tier is part of the SensorEvent schema (A'-SPEC §9.2). In A' all
/// sensors emit only `Hard`; Soft/Reputational arrive at stage E without API change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceTier {
    Hard,
}

/// A sensor alert surfaced to the user (Alerts pane / /alert command).
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// S1: message burst far above the contact's baseline.
    HighRate { from: String, msg_count: usize, median: f64 },
    /// S7: bundle arrived at an EID that was never handed to anyone.
    DecoyHit { source: String, decoy: String },
}

impl Alert {
    pub fn render(&self) -> String {
        match self {
            Alert::HighRate { from, msg_count, median } => {
                format!("S1 Alert: High rate from {from} ({msg_count} messages, median={median})")
            }
            Alert::DecoyHit { source, decoy } => {
                format!("S7 Alert: Decoy hit from {source} (decoy {decoy})")
            }
        }
    }
}

fn median(values: &[usize]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<usize> = values.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid] as f64
    } else {
        (sorted[mid - 1] + sorted[mid]) as f64 / 2.0
    }
}

/// Minimum burst size that can ever trigger S1. Without it the very first
/// single message (history empty, median = 0) would alert on `1 > 0`.
const MIN_BURST: usize = 10;

/// S1 ContactRateSensor (A'-SPEC §6.1): threshold-based for A', CUSUM is stage E.
/// `msg_count > median * 5` over a sliding window of the last 10 bursts.
pub struct ContactRateSensor {
    history: HashMap<PubKey, VecDeque<usize>>,
    window: usize,
}

impl ContactRateSensor {
    pub fn new() -> Self {
        ContactRateSensor { history: HashMap::new(), window: 10 }
    }

    /// Feed one burst of `msg_count` messages from `from`.
    pub fn on_burst(&mut self, from: &PubKey, msg_count: usize) -> Option<Alert> {
        if msg_count == 0 {
            return None;
        }
        let counts = self.history.entry(*from).or_default();
        let baseline = median(counts.make_contiguous());
        if msg_count >= MIN_BURST && msg_count as f64 > baseline * 5.0 {
            return Some(Alert::HighRate {
                from: short_key(from),
                msg_count,
                median: baseline,
            });
        }
        counts.push_back(msg_count);
        while counts.len() > self.window {
            counts.pop_front();
        }
        None
    }

    /// Feed a single message (burst of 1). Kept as a convenience wrapper.
    pub fn on_message(&mut self, from: &PubKey) -> Option<Alert> {
        self.on_burst(from, 1)
    }
}

impl Default for ContactRateSensor {
    fn default() -> Self {
        Self::new()
    }
}

/// Number of decoy EIDs kept active at any time.
const ACTIVE_DECOYS: usize = 3;

/// S7 DecoyEidSensor (A'-SPEC §6.2): honeypot. Decoys are generated locally as
/// blake3(random_nonce || node_pubkey || epoch) and "published" alongside the
/// node (in A': SSID/beacon; in the CLI: handshake payload). Any bundle
/// addressed to a decoy is by definition an attack — zero false positives.
pub struct DecoyEidSensor {
    decoys: Vec<[u8; 32]>,
    node_pubkey: PubKey,
    rotation_period: usize,
    contact_count: usize,
}

impl DecoyEidSensor {
    pub fn new(node_pubkey: PubKey) -> Self {
        let mut sensor = DecoyEidSensor {
            decoys: Vec::new(),
            node_pubkey,
            rotation_period: 10,
            contact_count: 0,
        };
        sensor.rotate();
        sensor
    }

    pub fn active_decoys(&self) -> &[[u8; 32]] {
        &self.decoys
    }

    /// Check an inbound bundle destination. Returns an alert on a decoy hit.
    pub fn on_bundle(&self, source: &PubKey, destination: &[u8; 32]) -> Option<Alert> {
        if self.decoys.contains(destination) {
            Some(Alert::DecoyHit {
                source: short_key(source),
                decoy: hex(destination),
            })
        } else {
            None
        }
    }

    /// Rotate decoys every `rotation_period` contacts.
    pub fn on_contact(&mut self) {
        self.contact_count += 1;
        if self.contact_count % self.rotation_period == 0 {
            self.rotate();
        }
    }

    fn rotate(&mut self) {
        let epoch = now_millis();
        self.decoys = (0..ACTIVE_DECOYS)
            .map(|_| generate_decoy(&self.node_pubkey, epoch))
            .collect();
    }
}

/// decoy = blake3(random_nonce || node_pubkey || epoch) (A'-SPEC §6.2).
pub fn generate_decoy(node_pubkey: &PubKey, epoch_ms: u128) -> [u8; 32] {
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&nonce);
    hasher.update(node_pubkey);
    hasher.update(&epoch_ms.to_le_bytes());
    hasher.finalize().into()
}

pub fn short_key(key: &PubKey) -> String {
    hex(key)[..12].to_string()
}

pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// One entry in the inbound buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingMessage {
    pub from: PubKey,
    pub text: String,
    pub received_at: SystemTime,
}

/// InboxBuffer (A'-SPEC §6.3): plain backpressure, oldest-first eviction by
/// count (100 messages) and by age (7 days). No shadow inbox, no FSM.
pub struct InboxBuffer {
    max_messages: usize,
    max_age: Duration,
    messages: VecDeque<IncomingMessage>,
}

impl InboxBuffer {
    pub fn new() -> Self {
        InboxBuffer {
            max_messages: 100,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            messages: VecDeque::new(),
        }
    }

    pub fn push(&mut self, msg: IncomingMessage) {
        self.evict_old();
        while self.messages.len() >= self.max_messages {
            self.messages.pop_front();
        }
        self.messages.push_back(msg);
    }

    pub fn evict_old(&mut self) {
        let cutoff = SystemTime::now()
            .checked_sub(self.max_age)
            .unwrap_or(UNIX_EPOCH);
        while self.messages.front().map(|m| m.received_at < cutoff).unwrap_or(false) {
            self.messages.pop_front();
        }
    }

    pub fn messages(&self) -> &VecDeque<IncomingMessage> {
        &self.messages
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

impl Default for InboxBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> PubKey {
        [seed; 32]
    }

    #[test]
    fn s1_flood_50_alerts_on_fresh_node() {
        let mut s1 = ContactRateSensor::new();
        let alert = s1.on_burst(&key(1), 50).expect("flood must alert");
        match alert {
            Alert::HighRate { from, msg_count, median } => {
                assert_eq!(msg_count, 50);
                assert_eq!(median, 0.0);
                assert!(!from.is_empty());
            }
            other => panic!("unexpected alert: {other:?}"),
        }
    }

    #[test]
    fn s1_single_messages_never_alert() {
        let mut s1 = ContactRateSensor::new();
        for _ in 0..20 {
            assert!(s1.on_message(&key(1)).is_none());
        }
    }

    #[test]
    fn s1_small_burst_below_min_never_alerts() {
        let mut s1 = ContactRateSensor::new();
        assert!(s1.on_burst(&key(1), 5).is_none());
        assert!(s1.on_burst(&key(1), 5).is_none());
    }

    #[test]
    fn s1_alerts_when_beat_median_by_5x() {
        let mut s1 = ContactRateSensor::new();
        // Baseline: five bursts of 6 -> median 6 (below MIN_BURST, never alerts alone).
        for _ in 0..5 {
            assert!(s1.on_burst(&key(1), 6).is_none());
        }
        assert!(s1.on_burst(&key(1), 31).is_some()); // 31 > 6*5
        // 30 messages does NOT alert (not strictly above 5x) but still gets recorded.
        assert!(s1.on_burst(&key(1), 30).is_none());
    }

    #[test]
    fn s1_tracks_contacts_independently() {
        let mut s1 = ContactRateSensor::new();
        for _ in 0..10 {
            assert!(s1.on_message(&key(1)).is_none());
            assert!(s1.on_message(&key(2)).is_none());
        }
        assert!(s1.on_burst(&key(1), 50).is_some());
        assert!(s1.on_burst(&key(2), 50).is_some());
    }

    #[test]
    fn s7_decoy_hit_alerts() {
        let sensor = DecoyEidSensor::new(key(9));
        let decoy = sensor.active_decoys()[0];
        let alert = sensor.on_bundle(&key(1), &decoy).expect("decoy hit must alert");
        match alert {
            Alert::DecoyHit { source, decoy: d } => {
                assert_eq!(source, short_key(&key(1)));
                assert_eq!(d, hex(&decoy));
            }
            other => panic!("unexpected alert: {other:?}"),
        }
    }

    #[test]
    fn s7_regular_destination_no_alert() {
        let sensor = DecoyEidSensor::new(key(9));
        assert!(sensor.on_bundle(&key(1), &[0xAA; 32]).is_none());
    }

    #[test]
    fn s7_rotation_replaces_decoys() {
        let mut sensor = DecoyEidSensor::new(key(9));
        let before = sensor.active_decoys().to_vec();
        for _ in 0..10 {
            sensor.on_contact();
        }
        let after = sensor.active_decoys().to_vec();
        assert_eq!(before.len(), after.len());
        assert!(before.iter().zip(after.iter()).all(|(a, b)| a != b));
    }

    #[test]
    fn s7_decoys_unique() {
        let sensor = DecoyEidSensor::new(key(9));
        let mut uniq = std::collections::HashSet::new();
        for d in sensor.active_decoys() {
            assert!(uniq.insert(*d));
        }
    }

    #[test]
    fn inbox_evicts_oldest_over_100() {
        let mut inbox = InboxBuffer::new();
        for i in 0..150 {
            inbox.push(IncomingMessage {
                from: key(1),
                text: format!("msg {i}"),
                received_at: SystemTime::now(),
            });
        }
        assert_eq!(inbox.len(), 100);
        assert_eq!(inbox.messages().front().unwrap().text, "msg 50");
        assert_eq!(inbox.messages().back().unwrap().text, "msg 149");
    }

    #[test]
    fn inbox_evicts_old_by_age() {
        let mut inbox = InboxBuffer::new();
        let eight_days = Duration::from_secs(8 * 24 * 60 * 60);
        inbox.push(IncomingMessage {
            from: key(1),
            text: "ancient".into(),
            received_at: SystemTime::now() - eight_days,
        });
        inbox.push(IncomingMessage {
            from: key(1),
            text: "fresh".into(),
            received_at: SystemTime::now(),
        });
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox.messages().front().unwrap().text, "fresh");
    }

    #[test]
    fn alert_render_matches_cli_spec() {
        let alert = Alert::HighRate { from: "Alice".into(), msg_count: 50, median: 0.0 };
        assert_eq!(alert.render(), "S1 Alert: High rate from Alice (50 messages, median=0)");
    }
}
