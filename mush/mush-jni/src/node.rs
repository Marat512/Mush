//! Node lifecycle behind the JNI handle (MUSH-A'-ANDROID-SPEC §5.1).
//!
//! The `extern "C"` entry points in `lib.rs` are thin string/exception
//! adapters over this inner API, which is `async` and sink-generic so host
//! tests can drive two nodes in one process without a JVM.
//!
//! Handle contract: `node_init` boxes a `JniNode` and leaks it as a pointer;
//! `node_shutdown` reclaims the box (aborting tasks; the Kotlin `GlobalRef`
//! dies with the sink). `0`/null handle is never valid.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use mush_core::base64;
use mush_core::transport::{Invite, Node, NodeEvent};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::callback::{spawn_dispatcher, EventSink};
use crate::runtime::runtime;

/// Error codes surfaced through `MushCallback.onError`.
pub const ERR_JOIN: i32 = 1;
pub const ERR_SEND: i32 = 2;

pub struct JniNode {
    node: Arc<Mutex<Node>>,
    sink: Arc<dyn EventSink>,
    tasks: Vec<JoinHandle<()>>,
    bound: bool,
    #[allow(dead_code)] // A' CLI core keeps state in memory; persistence lands later
    data_dir: String,
}

/// Create a node + its event dispatcher. Returns an owning raw pointer.
pub fn node_init(data_dir: &str, sink: Arc<dyn EventSink>) -> Result<*mut JniNode, String> {
    let mut node = Node::new("me", 0);
    let (tx, rx) = mpsc::unbounded_channel::<NodeEvent>();
    node.set_event_sink(tx);
    let boxed = Box::new(JniNode {
        node: Arc::new(Mutex::new(node)),
        sink: sink.clone(),
        tasks: vec![spawn_dispatcher(rx, sink)],
        bound: false,
        data_dir: data_dir.to_string(),
    });
    tracing::info!("mush-jni: node created");
    Ok(Box::into_raw(boxed))
}

/// Bind a listener, spawn the accept + S1-flush loops, return the actual port.
pub async unsafe fn node_start_listener(handle: *mut JniNode, port: u16) -> Result<u16, String> {
    let node = &mut *handle;
    if node.bound {
        return Err("listener already started".to_string());
    }
    let arc = node.node.clone();
    let (tx, rx) = oneshot::channel();
    let task = runtime().spawn(async move {
        let listener = match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(Err(format!("bind port {port}: {e}")));
                return;
            }
        };
        let bound = match listener.local_addr() {
            Ok(a) => a,
            Err(e) => {
                let _ = tx.send(Err(format!("local_addr: {e}")));
                return;
            }
        };
        {
            let mut n = arc.lock().await;
            // 127.0.0.1 placeholder: MS2 (Transport) replaces addr resolution
            // with the real hotspot/LAN interface IP.
            n.set_bound(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), bound.port()));
            n.set_self_ref(arc.clone());
        }
        let _ = tx.send(Ok(bound.port()));
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    let arc = arc.clone();
                    tokio::spawn(async move {
                        Node::handle_connection(arc, socket).await;
                    });
                }
                Err(e) => {
                    tracing::error!("accept failed: {e}");
                    break;
                }
            }
        }
    });
    node.tasks.push(task);

    // S1 flush tick (same cadence as the CLI's `run`).
    let arc = node.node.clone();
    node.tasks.push(runtime().spawn(async move {
        loop {
            let every = arc.lock().await.flush_every;
            sleep(every).await;
            arc.lock().await.flush_pending_bursts();
        }
    }));

    let port = timeout(Duration::from_secs(5), rx)
        .await
        .map_err(|_| "listener bind timed out".to_string())?
        .map_err(|_| "listener task died".to_string())??;
    node.bound = true;
    tracing::info!("mush-jni: listener started on 127.0.0.1:{port}");
    Ok(port)
}

/// Base64 Ed25519 pubkey (identity display / Safety Number precursor).
pub async unsafe fn node_pubkey(handle: *mut JniNode) -> Result<String, String> {
    let node = &*handle;
    let key = { node.node.lock().await.pubkey() };
    Ok(base64::encode(&key))
}

/// Full invite string (`Invite::render`, §5.0 of the Android spec) — this is
/// exactly what the QR code carries.
pub async unsafe fn node_invite(handle: *mut JniNode) -> Result<String, String> {
    let node = &*handle;
    Ok(node.node.lock().await.invite_now())
}

/// Parse-check the invite, then drive connect+handshake on the runtime.
/// Failures arrive asynchronously via `onError(ERR_JOIN, ...)`.
pub async unsafe fn node_join(handle: *mut JniNode, invite: &str) -> Result<bool, String> {
    let node = &mut *handle;
    // Sync validation so malformed links fail fast instead of as a callback.
    Invite::parse(invite)?;
    let arc = node.node.clone();
    let sink = node.sink.clone();
    let invite = invite.to_string();
    runtime().spawn(async move {
        if let Err(e) = arc.lock().await.join_peer(&invite).await {
            sink.on_error(ERR_JOIN, &e);
        }
    });
    Ok(true)
}

/// Debug hook: burst of `n` messages to trigger S1 (A'-SPEC §10, criterion 6).
pub async unsafe fn node_flood(handle: *mut JniNode, n: usize) -> Result<bool, String> {
    let node = &mut *handle;
    let arc = node.node.clone();
    let sink = node.sink.clone();
    runtime().spawn(async move {
        if let Err(e) = arc.lock().await.flood("default", n).await {
            sink.on_error(ERR_SEND, &e);
        }
    });
    Ok(true)
}

/// Debug hook: send a bundle to a peer's decoy EID to trigger S7 (criterion 7).
pub async unsafe fn node_decoy(handle: *mut JniNode) -> Result<bool, String> {
    let node = &mut *handle;
    let arc = node.node.clone();
    let sink = node.sink.clone();
    runtime().spawn(async move {
        if let Err(e) = arc.lock().await.send_decoy().await {
            sink.on_error(ERR_SEND, &e);
        }
    });
    Ok(true)
}

/// Этап A: режим криптографии (0 = Legacy, 1 = Ratchet). Android всегда
/// Ratchet (MUSH-A'-ANDROID-SPEC §13); вызывать до startListener.
pub async unsafe fn node_set_crypto_mode(handle: *mut JniNode, mode: i32) -> Result<(), String> {
    let node = &*handle;
    let m = match mode {
        0 => mush_core::transport::CryptoMode::Legacy,
        1 => mush_core::transport::CryptoMode::Ratchet,
        other => return Err(format!("bad crypto mode {other}")),
    };
    node.node.lock().await.set_crypto_mode(m);
    Ok(())
}

/// Override the address invites carry (hotspot/LAN interface IP). Empty/// string clears the override. Invites minted afterwards use it.
pub async unsafe fn node_set_advertised_addr(
    handle: *mut JniNode,
    addr: &str,
) -> Result<(), String> {
    let node = &*handle;
    let addr = if addr.is_empty() {
        None
    } else {
        Some(addr.to_string())
    };
    node.node.lock().await.set_advertised_addr(addr);
    Ok(())
}

/// TCP reachability probe (used by the UI to check a LAN address before join).
/// Synchronous on purpose: callers run it off the main thread.
pub unsafe fn node_connect(handle: *mut JniNode, host: &str, port: u16) -> Result<bool, String> {
    let _ = &*handle; // reserved for future connection reuse
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|_| format!("bad address '{host}:{port}'"))?;
    Ok(std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok())
}

/// Append a message to the working group and broadcast it.
/// Empty `group` = the joined/default group. Errors via `onError(ERR_SEND, ...)`.
pub async unsafe fn node_send(
    handle: *mut JniNode,
    group: &str,
    text: &str,
) -> Result<bool, String> {
    let node = &mut *handle;
    let prefix = if group.is_empty() { "default" } else { group };
    let arc = node.node.clone();
    let sink = node.sink.clone();
    let prefix = prefix.to_string();
    let text = text.to_string();
    runtime().spawn(async move {
        if let Err(e) = arc.lock().await.send_text(&prefix, &text).await {
            sink.on_error(ERR_SEND, &e);
        }
    });
    Ok(true)
}

/// Этап B: экспорт состояния ноды в CBOR (MUSH-B-SPEC §3). Работает и на
/// свежей ноде (экспортирует seed + pre-keys). Только явный user-action
/// (Backup/Transfer) — не onPause/onStop.
pub async unsafe fn node_export_state(handle: *mut JniNode) -> Result<Vec<u8>, String> {
    let node = &*handle;
    let state = node.node.lock().await.export_state();
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&state, &mut cbor).map_err(|e| format!("cbor encode: {e}"))?;
    Ok(cbor)
}

/// Этап B: замена ноды за handle из CBOR-состояния (MUSH-B-SPEC §3).
/// ВНИМАНИЕ: вызывать ТОЛЬКО на остановленной ноде — задачи слушателя
/// старой ноды обрываются, dispatcher пересоздаётся на новой. Ошибки
/// (невалидный CBOR, version mismatch, порча вложенного state) -> Ok(false),
/// не exception: Kotlin решает сам (например, fallback на новую identity).
pub async unsafe fn node_import_state(handle: *mut JniNode, bytes: &[u8]) -> Result<bool, String> {
    let node = &mut *handle;
    let Ok(state) = ciborium::de::from_reader::<mush_core::transport::NodeState, _>(bytes) else {
        return Ok(false);
    };
    let Some(mut imported) = mush_core::transport::Node::import_state(&state) else {
        return Ok(false);
    };
    // Старая нода уходит: обрываем её задачи (слушатель, flush, dispatcher).
    for task in node.tasks.drain(..) {
        task.abort();
    }
    // Новая нода на месте старой: канал событий + dispatcher заново,
    // sink и data_dir сохраняются (handle для Kotlin не меняется).
    let (tx, rx) = mpsc::unbounded_channel::<NodeEvent>();
    imported.set_event_sink(tx);
    node.node = Arc::new(Mutex::new(imported));
    node.tasks.push(spawn_dispatcher(rx, node.sink.clone()));
    node.bound = false;
    tracing::info!("mush-jni: node state imported");
    Ok(true)
}

/// Reclaim the node: abort spawned tasks, drop the sink (releases the Kotlin
/// GlobalRef). The handle must not be used afterwards.
pub unsafe fn node_shutdown(handle: *mut JniNode) {
    let node = Box::from_raw(handle);
    for task in node.tasks {
        task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callback::EventSink;
    use std::sync::Mutex;

    #[derive(Debug, Clone)]
    #[allow(dead_code)] // recorder keeps full events; not every field is asserted
    enum Record {
        Message { text: String },
        Handshake { name: String },
        Alert { sensor: String },
        Error { code: i32, msg: String },
    }

    #[derive(Default)]
    struct RecordingSink {
        log: Mutex<Vec<Record>>,
    }

    impl RecordingSink {
        fn has<F: Fn(&Record) -> bool>(&self, pred: F) -> bool {
            self.log.lock().unwrap().iter().any(pred)
        }
    }

    impl EventSink for RecordingSink {
        fn on_message(&self, _group: &[u8; 32], _sender: &[u8; 32], text: &str, _lamport: u64) {
            self.log.lock().unwrap().push(Record::Message {
                text: text.to_string(),
            });
        }
        fn on_sensor_alert(&self, sensor: &str, _target: &str, _reason: &str) {
            self.log.lock().unwrap().push(Record::Alert {
                sensor: sensor.to_string(),
            });
        }
        fn on_handshake_complete(&self, _peer: &[u8; 32], name: &str) {
            self.log.lock().unwrap().push(Record::Handshake {
                name: name.to_string(),
            });
        }
        fn on_error(&self, code: i32, message: &str) {
            self.log.lock().unwrap().push(Record::Error {
                code,
                msg: message.to_string(),
            });
        }
    }

    async fn wait_for<F: FnMut() -> bool>(mut cond: F, timeout_ms: u64, what: &str) {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if cond() {
                return;
            }
            sleep(Duration::from_millis(25)).await;
        }
        panic!("condition not met within {timeout_ms}ms: {what}");
    }

    /// Phase 1 acceptance (A'-ANDROID-SPEC): two nodes in one process,
    /// ports 18001/18002, handshake completes on both sides, send delivers,
    /// the receiving node's callback gets the message.
    #[test]
    fn two_nodes_handshake_send_and_callback() {
        runtime().block_on(async {
            let sink_a = Arc::new(RecordingSink::default());
            let sink_b = Arc::new(RecordingSink::default());
            let a = node_init("", sink_a.clone()).expect("init a");
            let b = node_init("", sink_b.clone()).expect("init b");

            let pa = unsafe { node_start_listener(a, 18001).await }.expect("bind a");
            let pb = unsafe { node_start_listener(b, 18002).await }.expect("bind b");
            assert_eq!((pa, pb), (18001, 18002));

            let key_a = unsafe { node_pubkey(a).await }.unwrap();
            assert_eq!(key_a.len(), 44); // base64 of 32 bytes

            let invite = unsafe { node_invite(a).await }.unwrap();
            assert!(invite.starts_with("mush://"));

            assert!(unsafe { node_join(b, &invite).await }.unwrap());
            wait_for(
                || sink_a.has(|r| matches!(r, Record::Handshake { name } if name == "me")),
                3000,
                "handshake event on a",
            )
            .await;
            wait_for(
                || sink_b.has(|r| matches!(r, Record::Handshake { name } if name == "me")),
                3000,
                "handshake event on b",
            )
            .await;

            assert!(unsafe { node_send(b, "", "hello alice").await }.unwrap());
            wait_for(
                || sink_a.has(|r| matches!(r, Record::Message { text } if text == "hello alice")),
                3000,
                "message callback on a",
            )
            .await;

            unsafe { node_shutdown(a) };
            unsafe { node_shutdown(b) };
        });
    }

    #[test]
    fn join_rejects_malformed_invite() {
        runtime().block_on(async {
            let sink = Arc::new(RecordingSink::default());
            let a = node_init("", sink).expect("init");
            let err = unsafe { node_join(a, "not-an-invite") }.await.unwrap_err();
            assert!(err.contains("mush://"), "unexpected error: {err}");
            unsafe { node_shutdown(a) };
        });
    }

    #[test]
    fn connect_probe_reports_unreachable() {
        runtime().block_on(async {
            let sink = Arc::new(RecordingSink::default());
            let a = node_init("", sink).expect("init");
            let ok = unsafe { node_connect(a, "127.0.0.1", 1) }.unwrap(); // port 1: closed
            assert!(!ok);
            unsafe { node_shutdown(a) };
        });
    }

    // ---- Этап B: export/import состояния (MUSH-B-SPEC §3) ----

    /// «Убийство процесса»: export -> байты (имитация записи в файл) ->
    /// import в свежую ноду -> identity/pubkey совпадают, pre-key store
    /// восстановлен (повторный export идентичен по содержимому).
    #[test]
    fn export_import_roundtrip_preserves_identity_and_prekeys() {
        runtime().block_on(async {
            let sink = Arc::new(RecordingSink::default());
            let a = node_init("", sink).expect("init");
            let pubkey_before = unsafe { node_pubkey(a).await }.unwrap();
            let cbor = unsafe { node_export_state(a) }.await.expect("export");
            unsafe { node_shutdown(a) };

            // Новая нода — как будто процесс перезапустился.
            let sink2 = Arc::new(RecordingSink::default());
            let b = node_init("", sink2).expect("init 2");
            assert_ne!(
                unsafe { node_pubkey(b).await }.unwrap(),
                pubkey_before,
                "до import — другая identity"
            );
            assert!(unsafe { node_import_state(b, &cbor) }.await.unwrap());

            assert_eq!(
                unsafe { node_pubkey(b).await }.unwrap(),
                pubkey_before,
                "после import — исходный pubkey"
            );
            // Повторный export совпадает побайтово: seed, pre-keys, OPK-очередь
            // (used tombstones с wall-clock-таймстемпами в этом тесте пусты).
            let cbor2 = unsafe { node_export_state(b) }.await.expect("re-export");
            assert_eq!(cbor, cbor2, "export после import должен быть идентичен");
            unsafe { node_shutdown(b) };
        });
    }

    /// Идентикон и safety number от восстановленной identity совпадают
    /// с исходными — ловит дрейф export/import на уровне UI-функций.
    #[test]
    fn export_import_preserves_identicon_and_safety_number() {
        runtime().block_on(async {
            let sink = Arc::new(RecordingSink::default());
            let a = node_init("", sink).expect("init");
            let sink_b = Arc::new(RecordingSink::default());
            let b = node_init("", sink_b).expect("init b");
            let pk_a = unsafe { node_pubkey(a).await }.unwrap();
            let pk_b = unsafe { node_pubkey(b).await }.unwrap();
            let icon_before = crate::pure::identicon_svg(&pk_a).unwrap();
            let safety_before = crate::pure::safety_number(&pk_a, &pk_b).unwrap();
            let cbor_a = unsafe { node_export_state(a) }.await.expect("export a");
            let cbor_b = unsafe { node_export_state(b) }.await.expect("export b");
            unsafe { node_shutdown(a) };
            unsafe { node_shutdown(b) };

            let sink2 = Arc::new(RecordingSink::default());
            let a2 = node_init("", sink2).expect("init 2");
            let sink2b = Arc::new(RecordingSink::default());
            let b2 = node_init("", sink2b).expect("init 2b");
            assert!(unsafe { node_import_state(a2, &cbor_a) }.await.unwrap());
            assert!(unsafe { node_import_state(b2, &cbor_b) }.await.unwrap());

            let pk_a2 = unsafe { node_pubkey(a2).await }.unwrap();
            let pk_b2 = unsafe { node_pubkey(b2).await }.unwrap();
            assert_eq!(crate::pure::identicon_svg(&pk_a2).unwrap(), icon_before);
            assert_eq!(
                crate::pure::safety_number(&pk_a2, &pk_b2).unwrap(),
                safety_before
            );
            unsafe { node_shutdown(a2) };
            unsafe { node_shutdown(b2) };
        });
    }

    #[test]
    fn import_rejects_garbage_and_keeps_node() {
        runtime().block_on(async {
            let sink = Arc::new(RecordingSink::default());
            let a = node_init("", sink).expect("init");
            let pubkey_before = unsafe { node_pubkey(a).await }.unwrap();
            // Мусор — false, нода не тронута.
            assert!(!unsafe { node_import_state(a, b"\xde\xad\xbe\xef") }
                .await
                .unwrap());
            // Валидный CBOR, но version mismatch — тоже false.
            let mut state = {
                let cbor = unsafe { node_export_state(a) }.await.expect("export");
                ciborium::de::from_reader::<mush_core::transport::NodeState, _>(cbor.as_slice())
                    .expect("decode")
            };
            state.version = 99;
            let mut bad = Vec::new();
            ciborium::ser::into_writer(&state, &mut bad).expect("encode");
            assert!(!unsafe { node_import_state(a, &bad) }.await.unwrap());
            assert_eq!(
                unsafe { node_pubkey(a).await }.unwrap(),
                pubkey_before,
                "нода осталась прежней"
            );
            unsafe { node_shutdown(a) };
        });
    }
}

#[cfg(test)]
mod crypto_mode_tests {
    use super::*;
    use crate::callback::EventSink;

    struct Noop;
    impl EventSink for Noop {
        fn on_message(&self, _: &[u8; 32], _: &[u8; 32], _: &str, _: u64) {}
        fn on_sensor_alert(&self, _: &str, _: &str, _: &str) {}
        fn on_handshake_complete(&self, _: &[u8; 32], _: &str) {}
        fn on_error(&self, _: i32, _: &str) {}
    }

    fn noop() -> Arc<Noop> {
        Arc::new(Noop)
    }

    #[test]
    fn set_crypto_mode_ratchet_makes_invite_carry_bundle() {
        runtime().block_on(async {
            let sink = noop();
            let a = node_init("", sink.clone()).expect("init");
            unsafe { node_set_crypto_mode(a, 1).await }.unwrap();
            unsafe { node_start_listener(a, 18441).await }.unwrap();
            let invite = unsafe { node_invite(a).await }.unwrap();
            assert!(invite.contains("pk="), "ratchet invite несёт pk= bundle");
            unsafe { node_shutdown(a) };
        });
    }

    #[test]
    fn set_crypto_mode_rejects_bad_value() {
        runtime().block_on(async {
            let sink = noop();
            let a = node_init("", sink).expect("init");
            let err = unsafe { node_set_crypto_mode(a, 7) }.await.unwrap_err();
            assert!(err.contains("bad crypto mode"));
            unsafe { node_shutdown(a) };
        });
    }
}
