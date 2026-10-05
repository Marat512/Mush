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
        // Ленивая загрузка: Android грузит "mush_jni" из APK-jniLibs, JVM-тесты
        // (IdenticonGoldenTest) — конкретный файл через System.load ДО первого
        // вызова (путь из env MUSH_JNI_LIB). runCatching: в JVM-тесте
        // loadLibrary упадёт — это допустимо, пока тест сам подгрузил либу.
        runCatching { System.loadLibrary("mush_jni") }
    }

    external fun init(dataDir: String, callback: MushCallback): Long
    external fun pubkey(handle: Long): String
    external fun invite(handle: Long): String
    external fun join(handle: Long, invite: String): Boolean
    external fun startListener(handle: Long, port: Int): Int
    external fun connect(handle: Long, host: String, port: Int): Boolean
    external fun send(handle: Long, groupId: String, text: String): Boolean
    external fun setAdvertisedAddr(handle: Long, addr: String)
    // Debug hooks для §10 (критерии 6/7): триггер S1/S7 вручную.
    external fun flood(handle: Long, n: Int): Boolean
    external fun decoy(handle: Long): Boolean
    // Этап A: 0 = Legacy, 1 = Ratchet. Android всегда 1 (§13).
    external fun setCryptoMode(handle: Long, mode: Int)

    // --- Этап B (MUSH-B-SPEC §3) ---
    // Handle-free: идентикон/safety number рисуются ДО старта ноды
    // (onboarding, сверка ключей) — handle не нужен. Вход — base64(pubkey),
    // как у [pubkey]. Rust — единственный источник правды рендеринга:
    // Kotlin не рендерит идентикон из bytes для UI (bytes — только
    // golden-vector сверка Rust↔Kotlin). Невалидный base64/длина != 32
    // бросают IllegalArgumentException.
    external fun identiconSvg(pubkey: String): String
    external fun identiconBytes(pubkey: String): ByteArray
    external fun safetyNumber(a: String, b: String): String

    // Явный Backup/Transfer состояния ноды — ТОЛЬКО user-action, НЕ
    // onPause/onStop. exportState работает и на свежей ноде (seed+pre-keys).
    // importState ЗАМЕНЯЕТ ноду за handle: вызывать ТОЛЬКО на остановленной
    // ноде (задачи старой обрываются). Невалидный CBOR/version mismatch ->
    // false, не exception — Kotlin решает (fallback на новую identity).
    external fun exportState(handle: Long): ByteArray
    external fun importState(handle: Long, bytes: ByteArray): Boolean
    external fun shutdown(handle: Long)
}
