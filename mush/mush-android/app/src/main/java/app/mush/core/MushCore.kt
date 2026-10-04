package app.mush.core

/**
 * JNI bridge to mush-core (Rust). Phase 0 (MS0): proof of life.
 *
 * The native library is built by cargo-ndk into app/src/main/jniLibs:
 *   cargo ndk -t arm64-v8a -t armv7 -t x86_64 -o app/src/main/jniLibs build --release
 * (see MUSH-A'-ANDROID-SPEC.md, Фаза 0).
 */
object MushCore {
    init {
        System.loadLibrary("mush_jni")
    }

    external fun hello(): String
}
