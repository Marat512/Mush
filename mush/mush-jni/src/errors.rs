//! JNI error plumbing (MUSH-A'-ANDROID-SPEC §5.5).

use jni::JNIEnv;

/// Throw `java/lang/RuntimeException` with `msg`. Returns nothing; the JNI
/// entry point then returns a default value so the JVM sees the exception.
pub fn throw(env: &mut JNIEnv, msg: &str) {
    let _ = env.throw_new("java/lang/RuntimeException", msg);
}
