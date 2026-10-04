//! JNI bridge between Kotlin (`app.mush.Native`) and `mush-core`
//! (MUSH-A'-ANDROID-SPEC §5).
//!
//! Every `extern "C"` entry point is panic-proof: a native panic across the
//! FFI boundary aborts the JVM process, so all calls run under `catch_unwind`
//! and rethrow as `java/lang/RuntimeException`. The real logic lives in
//! `node.rs` (inner API, async, sink-generic) so host tests exercise exactly
//! what the JVM calls.

mod callback;
mod errors;
mod node;
mod runtime;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;

use callback::{EventSink, KotlinCallback};
use errors::throw;
use node::{
    node_connect, node_init, node_invite, node_join, node_pubkey, node_send,
    node_set_advertised_addr, node_shutdown, node_start_listener,
};
use runtime::runtime;

/// Install a logcat writer for `tracing` events on Android (MS0: Rust logs
/// visible in Logcat). No-op on host builds — tracing-android only compiles
/// for the Android target.
#[cfg(target_os = "android")]
fn init_tracing_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        use tracing_subscriber::filter::{LevelFilter, Targets};
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(tracing_android::layer("mush").expect("android logcat layer"))
            // Only Mush crates: the `jni` crate trace-spams per JNI call.
            .with(
                Targets::new()
                    .with_target("mush_core", LevelFilter::TRACE)
                    .with_target("mush_jni", LevelFilter::TRACE),
            )
            .init();
    });
}

#[cfg(not(target_os = "android"))]
fn init_tracing_once() {}

fn get_string(env: &mut JNIEnv, s: &JString) -> Result<String, String> {
    env.get_string(s)
        .map(|js| js.into())
        .map_err(|e| format!("bad Java string: {e}"))
}

fn to_jstring(env: &mut JNIEnv, s: String) -> jstring {
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_init(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
    callback: JObject,
) -> jlong {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jlong, String> {
        let data_dir = get_string(&mut env, &data_dir)?;
        let sink: Arc<dyn EventSink> = Arc::new(
            KotlinCallback::new(&mut env, callback).map_err(|e| format!("callback ref: {e}"))?,
        );
        init_tracing_once();
        let handle = node_init(&data_dir, sink)?;
        Ok(handle as jlong)
    }));
    match result {
        Ok(Ok(handle)) => handle,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.init");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_pubkey<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        runtime().block_on(unsafe { node_pubkey(handle as *mut node::JniNode) })
    }));
    match result {
        Ok(Ok(s)) => to_jstring(&mut env, s),
        Ok(Err(e)) => {
            throw(&mut env, &e);
            std::ptr::null_mut()
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.pubkey");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_invite<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        runtime().block_on(unsafe { node_invite(handle as *mut node::JniNode) })
    }));
    match result {
        Ok(Ok(s)) => to_jstring(&mut env, s),
        Ok(Err(e)) => {
            throw(&mut env, &e);
            std::ptr::null_mut()
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.invite");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_join(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    invite: JString,
) -> jboolean {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        let invite = get_string(&mut env, &invite)?;
        Ok(
            runtime().block_on(unsafe { node_join(handle as *mut node::JniNode, &invite) })?
                as jboolean,
        )
    }));
    match result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.join");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_startListener(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    port: jint,
) -> jint {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jint, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        if !(0..=65535).contains(&port) {
            return Err(format!("bad port {port}"));
        }
        let actual = runtime()
            .block_on(unsafe { node_start_listener(handle as *mut node::JniNode, port as u16) })?;
        Ok(actual as jint)
    }));
    match result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.startListener");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_connect(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    host: JString,
    port: jint,
) -> jboolean {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        if !(0..=65535).contains(&port) {
            return Err(format!("bad port {port}"));
        }
        let host = get_string(&mut env, &host)?;
        Ok(unsafe { node_connect(handle as *mut node::JniNode, &host, port as u16) }? as jboolean)
    }));
    match result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.connect");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_send(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    group_id: JString,
    text: JString,
) -> jboolean {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        let group_id = get_string(&mut env, &group_id)?;
        let text = get_string(&mut env, &text)?;
        Ok(runtime()
            .block_on(unsafe { node_send(handle as *mut node::JniNode, &group_id, &text) })?
            as jboolean)
    }));
    match result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.send");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_setAdvertisedAddr(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    addr: JString,
) {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        let addr = get_string(&mut env, &addr)?;
        runtime().block_on(unsafe { node_set_advertised_addr(handle as *mut node::JniNode, &addr) })
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => throw(&mut env, &e),
        Err(_) => throw(&mut env, "Rust panic in Native.setAdvertisedAddr"),
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_shutdown(mut env: JNIEnv, _class: JClass, handle: jlong) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        unsafe { node_shutdown(handle as *mut node::JniNode) };
        Ok(())
    }));
    if let Ok(Err(e)) = result {
        throw(&mut env, &e);
    } else if result.is_err() {
        throw(&mut env, "Rust panic in Native.shutdown");
    }
}
