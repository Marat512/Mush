package app.mush

/** Rust -> Kotlin events (MUSH-A'-ANDROID-SPEC §5.2). Implemented by the
 * Service-level object; Rust holds a GlobalRef and calls these from its own
 * threads — implementations must hand off to the main thread themselves. */
interface MushCallback {
    fun onMessage(groupId: String, senderPubkey: String, text: String, ts: Long)
    fun onSensorAlert(sensor: String, targetPubkey: String, reason: String)
    fun onHandshakeComplete(peerPubkey: String)
    fun onError(code: Int, message: String)
}

/** JNI entry points over `mush-core` (§5.1). `handle` is an opaque native
 * pointer obtained from [init]; pass it to every other call, finish with
 * [shutdown]. Blocking calls ([startListener], [connect]) must run off the
 * main thread. */
object Native {
    init {
        System.loadLibrary("mush_jni")
    }

    external fun init(dataDir: String, callback: MushCallback): Long
    external fun pubkey(handle: Long): String
    external fun invite(handle: Long): String
    external fun join(handle: Long, invite: String): Boolean
    external fun startListener(handle: Long, port: Int): Int
    external fun connect(handle: Long, host: String, port: Int): Boolean
    external fun send(handle: Long, groupId: String, text: String): Boolean
    external fun setAdvertisedAddr(handle: Long, addr: String)
    external fun shutdown(handle: Long)
}
