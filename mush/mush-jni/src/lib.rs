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
mod pure;
mod runtime;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;

use callback::{EventSink, KotlinCallback};
use errors::{throw, throw_illegal_argument};
use node::{
    node_connect, node_decoy, node_export_state, node_flood, node_import_state, node_init,
    node_invite, node_join, node_pubkey, node_send, node_set_advertised_addr, node_set_crypto_mode,
    node_shutdown, node_start_listener,
};
use pure::{identicon_bytes, identicon_svg, safety_number};
use runtime::block_on_reentrant;

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
        // ABI загруженной .so: run-as app.mush + /proc/self/maps не всегда
        // доступен проверяющему — фиксируем при загрузке либы (§3.5 п.15:
        // Redmi 5 может тянуть armeabi-v7a .so вместо arm64).
        tracing::info!("mush-jni: init (arch {})", std::env::consts::ARCH);
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
        block_on_reentrant(unsafe { node_pubkey(handle as *mut node::JniNode) })
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
        block_on_reentrant(unsafe { node_invite(handle as *mut node::JniNode) })
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
            block_on_reentrant(unsafe { node_join(handle as *mut node::JniNode, &invite) })?
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
        let actual = block_on_reentrant(unsafe {
            node_start_listener(handle as *mut node::JniNode, port as u16)
        })?;
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
    let result =
        catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
            if handle == 0 {
                return Err("null handle".to_string());
            }
            let group_id = get_string(&mut env, &group_id)?;
            let text = get_string(&mut env, &text)?;
            Ok(block_on_reentrant(unsafe {
                node_send(handle as *mut node::JniNode, &group_id, &text)
            })? as jboolean)
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
        block_on_reentrant(unsafe { node_set_advertised_addr(handle as *mut node::JniNode, &addr) })
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => throw(&mut env, &e),
        Err(_) => throw(&mut env, "Rust panic in Native.setAdvertisedAddr"),
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_flood(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    n: jint,
) -> jboolean {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        Ok(block_on_reentrant(unsafe {
            node_flood(handle as *mut node::JniNode, n.max(0) as usize)
        })? as jboolean)
    }));
    match result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.flood");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_decoy(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        Ok(block_on_reentrant(unsafe { node_decoy(handle as *mut node::JniNode) })? as jboolean)
    }));
    match result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.decoy");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_setCryptoMode(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    mode: jint,
) {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        block_on_reentrant(unsafe { node_set_crypto_mode(handle as *mut node::JniNode, mode) })
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => throw(&mut env, &e),
        Err(_) => throw(&mut env, "Rust panic in Native.setCryptoMode"),
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

/// jbyteArray из среза; при нехватке памяти JVM — null + исключение.
fn to_jbyte_array(env: &mut JNIEnv, bytes: &[u8]) -> jni::sys::jbyteArray {
    match env.byte_array_from_slice(bytes) {
        Ok(arr) => arr.into_raw(),
        Err(_) => {
            throw(env, "byte_array_from_slice failed");
            std::ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Этап B (MUSH-B-SPEC §3): handle-free UI-функции. Идентикон/safety number
// рисуются ДО старта ноды (onboarding, сверка ключей) — handle не нужен.
// Rust — единственный источник правды рендеринга: Kotlin не рендерит
// идентикон из bytes для UI (bytes — только golden-vector сверка).
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_identiconSvg(
    mut env: JNIEnv,
    _class: JClass,
    pubkey_b64: JString,
) -> jstring {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let pk = get_string(&mut env, &pubkey_b64)?;
        identicon_svg(&pk)
    }));
    match result {
        Ok(Ok(s)) => to_jstring(&mut env, s),
        Ok(Err(e)) => {
            throw_illegal_argument(&mut env, &e);
            std::ptr::null_mut()
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.identiconSvg");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_identiconBytes(
    mut env: JNIEnv,
    _class: JClass,
    pubkey_b64: JString,
) -> jni::sys::jbyteArray {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, String> {
        let pk = get_string(&mut env, &pubkey_b64)?;
        identicon_bytes(&pk)
    }));
    match result {
        Ok(Ok(bytes)) => to_jbyte_array(&mut env, &bytes),
        Ok(Err(e)) => {
            throw_illegal_argument(&mut env, &e);
            std::ptr::null_mut()
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.identiconBytes");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_safetyNumber(
    mut env: JNIEnv,
    _class: JClass,
    pk_a_b64: JString,
    pk_b_b64: JString,
) -> jstring {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let a = get_string(&mut env, &pk_a_b64)?;
        let b = get_string(&mut env, &pk_b_b64)?;
        safety_number(&a, &b)
    }));
    match result {
        Ok(Ok(s)) => to_jstring(&mut env, s),
        Ok(Err(e)) => {
            throw_illegal_argument(&mut env, &e);
            std::ptr::null_mut()
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.safetyNumber");
            std::ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Этап B (MUSH-B-SPEC §3): явный Backup/Transfer состояния ноды.
// ТОЛЬКО user-action — не onPause/onStop. importState заменяет ноду за
// handle: вызывать ТОЛЬКО на остановленной ноде (задачи старой обрываются).
// Невалидный CBOR/version mismatch -> false, не exception (Kotlin решает:
// fallback на новую identity).
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_exportState(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jbyteArray {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        block_on_reentrant(unsafe { node_export_state(handle as *mut node::JniNode) })
    }));
    match result {
        Ok(Ok(bytes)) => to_jbyte_array(&mut env, &bytes),
        Ok(Err(e)) => {
            throw(&mut env, &e);
            std::ptr::null_mut()
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.exportState");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn Java_app_mush_Native_importState(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    bytes: JByteArray,
) -> jboolean {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jboolean, String> {
        if handle == 0 {
            return Err("null handle".to_string());
        }
        let bytes = env
            .convert_byte_array(&bytes)
            .map_err(|e| format!("bad byte array: {e}"))?;
        let ok =
            block_on_reentrant(unsafe { node_import_state(handle as *mut node::JniNode, &bytes) })?;
        Ok(ok as jboolean)
    }));
    match result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            throw(&mut env, &e);
            0
        }
        Err(_) => {
            throw(&mut env, "Rust panic in Native.importState");
            0
        }
    }
}
