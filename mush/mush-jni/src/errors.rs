//! JNI error plumbing (MUSH-A'-ANDROID-SPEC §5.5).

use jni::JNIEnv;

/// Throw `java/lang/RuntimeException` with `msg`. Returns nothing; the JNI
/// entry point then returns a default value so the JVM sees the exception.
pub fn throw(env: &mut JNIEnv, msg: &str) {
    let _ = env.throw_new("java/lang/RuntimeException", msg);
}

/// Throw `java/lang/IllegalArgumentException` — для handle-free функций
/// (MUSH-B-SPEC §3): невалидный base64/pubkey в identicon/safety number
/// это ошибка вызывающего кода, а не ноды.
pub fn throw_illegal_argument(env: &mut JNIEnv, msg: &str) {
    let _ = env.throw_new("java/lang/IllegalArgumentException", msg);
}
