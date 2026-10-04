//! JNI bridge between Kotlin (`app.mush.core.MushCore`) and `mush-core`.
//!
//! Phase 0 (MS0): prove Rust executes inside the APK — one `hello()` function.
//! Phases 1+: full node lifecycle bridge (MUSH-A'-ANDROID-SPEC §Фаза 1).
//!
//! Every extern "C" entry point must be panic-proof: a native panic across
//! the FFI boundary aborts the JVM process. Phase 0 keeps one safe entry
//! point; the full bridge wraps all calls in `catch_unwind` and rethrows
//! as Java exceptions.

use jni::objects::{JClass, JString};
use jni::JNIEnv;

/// Pure logic, unit-testable without a JVM.
fn hello_payload() -> String {
    let identity = mush_core::identity::Identity::generate();
    let key = mush_core::base64::encode(&identity.pubkey_bytes());
    format!("Hello from Rust! Ed25519 key: {key}")
}

#[no_mangle]
pub extern "C" fn Java_app_mush_core_MushCore_hello<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    // Identity::generate is infallible in practice (OsRng); if it ever
    // panicked, the process would die — so guard the boundary now.
    let payload = std::panic::catch_unwind(hello_payload)
        .unwrap_or_else(|_| "Rust panic in hello()".to_string());
    env.new_string(payload)
        .expect("failed to allocate JVM string")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_contains_rust_marker_and_key() {
        let p = hello_payload();
        assert!(p.starts_with("Hello from Rust! Ed25519 key: "));
        let key = p.trim_start_matches("Hello from Rust! Ed25519 key: ");
        // Ed25519 pubkey, base64 of 32 bytes = 44 chars with padding.
        assert_eq!(key.len(), 44);
        assert!(key.ends_with('='));
    }

    #[test]
    fn payload_keys_are_unique() {
        assert_ne!(hello_payload(), hello_payload());
    }
}
