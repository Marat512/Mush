//! Rust -> Kotlin event bridge (MUSH-A'-ANDROID-SPEC §5.3).
//!
//! `mush-core` pushes `NodeEvent`s over an unbounded channel; this module
//! drains it through a bounded buffer (drop-oldest under an event storm, so a
//! flooding peer cannot grow the heap without limit) into an `EventSink`.
//!
//! Two sink implementations exist:
//! - `KotlinCallback` — real JNI bridge: holds `JavaVM` + `GlobalRef` to the
//!   Kotlin `MushCallback` object, `attach_current_thread` per event, and the
//!   `GlobalRef` is released when the sink is dropped (in `shutdown`).
//! - test `RecordingSink` — proves the wiring without a JVM.

use std::collections::VecDeque;
use std::sync::Arc;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};
use mush_core::base64;
use mush_core::sensors::Alert;
use mush_core::transport::NodeEvent;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::runtime::runtime;

/// Bounded event buffer: under an event storm (e.g. S1 flooding) the oldest
/// pending events are dropped, never the heap.
const EVENT_CAP: usize = 256;

/// Receives core events in typed form. Object-safe so tests can substitute a
/// recording sink for the real JNI callback.
pub trait EventSink: Send + Sync + 'static {
    fn on_message(&self, group: &[u8; 32], sender: &[u8; 32], text: &str, lamport: u64);
    fn on_sensor_alert(&self, sensor: &str, target_pubkey: &str, reason: &str);
    fn on_handshake_complete(&self, peer: &[u8; 32], name: &str);
    fn on_error(&self, code: i32, message: &str);
}

/// Drain core events into the sink. Spawned per node in `node_init`.
pub fn spawn_dispatcher(
    mut rx: mpsc::UnboundedReceiver<NodeEvent>,
    sink: Arc<dyn EventSink>,
) -> JoinHandle<()> {
    runtime().spawn(async move {
        let mut buffer: VecDeque<NodeEvent> = VecDeque::with_capacity(EVENT_CAP);
        while let Some(event) = rx.recv().await {
            buffer.push_back(event);
            if buffer.len() > EVENT_CAP {
                buffer.pop_front(); // drop oldest under storm
            }
            while let Some(event) = buffer.pop_front() {
                dispatch(&sink, event);
            }
        }
    })
}

fn dispatch(sink: &Arc<dyn EventSink>, event: NodeEvent) {
    match event {
        NodeEvent::Message {
            group,
            sender,
            text,
            lamport,
        } => sink.on_message(&group, &sender, &text, lamport),
        NodeEvent::Alert(alert) => {
            let (sensor, target) = match &alert {
                Alert::HighRate { from, .. } => ("S1", from.clone()),
                Alert::DecoyHit { source, .. } => ("S7", source.clone()),
            };
            sink.on_sensor_alert(sensor, &target, &alert.render());
        }
        NodeEvent::HandshakeComplete { peer, name } => sink.on_handshake_complete(&peer, &name),
    }
}

/// JNI-backed sink: calls the Kotlin `MushCallback` methods.
///
/// Lifecycle (§5.3): owns a `GlobalRef` created in `init`, released on drop
/// (`shutdown`). Never references an Activity — the callback object must
/// belong to the Service.
pub struct KotlinCallback {
    vm: JavaVM,
    obj: GlobalRef,
}

impl KotlinCallback {
    pub fn new(env: &mut JNIEnv, callback: JObject) -> Result<Self, jni::errors::Error> {
        let vm = env.get_java_vm()?;
        let obj = env.new_global_ref(callback)?;
        Ok(KotlinCallback { vm, obj })
    }
}

impl EventSink for KotlinCallback {
    fn on_message(&self, group: &[u8; 32], sender: &[u8; 32], text: &str, lamport: u64) {
        // attach_current_thread per call — Android threads come and go; a
        // JNIEnv must never be cached (§5.3).
        let Ok(mut env) = self.vm.attach_current_thread() else {
            return;
        };
        let (Ok(g), Ok(s), Ok(t)) = (
            env.new_string(base64::encode(group)),
            env.new_string(base64::encode(sender)),
            env.new_string(text),
        ) else {
            return;
        };
        let (g, s, t): (JObject, JObject, JObject) = (g.into(), s.into(), t.into());
        let args = [
            JValue::Object(&g),
            JValue::Object(&s),
            JValue::Object(&t),
            JValue::Long(lamport as i64),
        ];
        let _ = env.call_method(
            &self.obj,
            "onMessage",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)V",
            &args,
        );
    }

    fn on_sensor_alert(&self, sensor: &str, target_pubkey: &str, reason: &str) {
        let Ok(mut env) = self.vm.attach_current_thread() else {
            return;
        };
        let (Ok(sn), Ok(tp), Ok(r)) = (
            env.new_string(sensor),
            env.new_string(target_pubkey),
            env.new_string(reason),
        ) else {
            return;
        };
        let (sn, tp, r): (JObject, JObject, JObject) = (sn.into(), tp.into(), r.into());
        let args = [JValue::Object(&sn), JValue::Object(&tp), JValue::Object(&r)];
        let _ = env.call_method(
            &self.obj,
            "onSensorAlert",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
            &args,
        );
    }

    fn on_handshake_complete(&self, peer: &[u8; 32], _name: &str) {
        // §5.2: onHandshakeComplete(peerPubkey) — один аргумент.
        let Ok(mut env) = self.vm.attach_current_thread() else {
            return;
        };
        let Ok(p) = env.new_string(base64::encode(peer)) else {
            return;
        };
        let p: JObject = p.into();
        let args = [JValue::Object(&p)];
        let _ = env.call_method(
            &self.obj,
            "onHandshakeComplete",
            "(Ljava/lang/String;)V",
            &args,
        );
    }

    fn on_error(&self, code: i32, message: &str) {
        let Ok(mut env) = self.vm.attach_current_thread() else {
            return;
        };
        let Ok(m) = env.new_string(message) else {
            return;
        };
        let m: JObject = m.into();
        let args = [JValue::Int(code as jint), JValue::Object(&m)];
        let _ = env.call_method(&self.obj, "onError", "(ILjava/lang/String;)V", &args);
    }
}
