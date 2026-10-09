package app.mush

import android.content.Context
import net.zetetic.database.sqlcipher.SQLiteDatabase
import java.io.File
import java.security.SecureRandom

/** Запись истории из Vault (MUSH-B-SPEC §1.3). */
data class VaultMessage(
    val id: Long,
    val groupId: ByteArray,
    val sender: ByteArray,
    val lamport: Long,
    val text: String,
    val createdAt: Long,
)

/** Уровень доверия контакта (MUSH-B-SPEC §3.1/§3.2).
 *
 * `verification_failed` — ТЕКСТ-значение в существующей колонке contacts.
 * trust_level: DDL-миграция НЕ нужна, schema_version остаётся 1 — значение
 * добавочное (§3.2 зафиксировано до кода). Verified ставит «Совпадает»,
 * VerificationFailed — только явное «Не совпадает»; снимается только явной
 * повторной сверкой. */
enum class TrustLevel {
    UNVERIFIED,
    VERIFIED,
    VERIFICATION_FAILED,
    ;

    fun toDbValue(): String = when (this) {
        UNVERIFIED -> "unverified"
        VERIFIED -> "verified"
        VERIFICATION_FAILED -> "verification_failed"
    }

    companion object {
        fun fromDbValue(value: String): TrustLevel =
            entries.firstOrNull { it.name.equals(value, ignoreCase = true) } ?: UNVERIFIED
    }
}

/** Vault: SQLCipher-БД узла (MUSH-B-SPEC §1, §7 дни 4–5).
 *
 * DEK — случайные 32 байта, шифрует БД (PRAGMA key); хранится обёрнутым
 * через [DekProvider] в `files/vault/dek.bin`. Провайдер сейчас plaintext
 * ([PlaintextDekProvider]) — замена на Keystore-обёртку гейтится
 * hardware-verify §6, это НЕ баг.
 *
 * Threading: простая синхронизация (@Synchronized) — конкурентный доступ
 * сложнее Mutex в B не нужен (спека §7).
 *
 * НЕ путать с user-facing Backup: это внутренняя persistence, пишется по
 * throttled-событиям из сервиса. Флаг «export/import только явное действие»
 * (MUSH-B-SPEC §4) про бэкап в файл — здесь не нарушается. */
class VaultManager private constructor(
    private val db: SQLiteDatabase,
) {
    companion object {
        private const val KEY_NODE_STATE = "node_state"

        /** Открыть (или создать) Vault: files/vault/mush.db с миграциями до
         * schema_version=1. [dekProvider] — временно plaintext, см. класс.
         *
         * Ошибки DEK НЕ глотаем: unwrap-Result прокидывается наружу как
         * бросок [DekError] (он же Exception). Граница Result — VaultManager:
         * MushService ловит DekError и решает (временное поведение до §1.2.1:
         * работа без Vault, identity эфемерна). */
        @JvmStatic
        fun open(context: Context, dekProvider: DekProvider = PlaintextDekProvider()): VaultManager {
            // sqlcipher-android 4.17: классы AAR не грузят нативную либу сами
            // (нет loadLibs в API) — грузим явно. Только здесь, в Android-пути:
            // companion-init грузил бы либу и в JVM-тестах (UnsatisfiedLinkError).
            System.loadLibrary("sqlcipher")
            val dir = File(context.filesDir, "vault").apply { mkdirs() }
            val dek = loadOrCreateDek(File(dir, "dek.bin"), dekProvider).getOrThrow()
            // Нативная либа грузится статическим инициализатором SQLCipher
            // 4.17+ (loadLibs убран из API). Raw 32-байтовый DEK — PRAGMA key
            // эквивалент на уровне API.
            val db = SQLiteDatabase.openOrCreateDatabase(
                File(dir, "mush.db"),
                dek,
                null,
                null,
            )
            migrate(db)
            return VaultManager(db)
        }

        /** DEK: есть dek.bin -> unwrap; нет -> сгенерировать, wrap, записать.
         * Файл пишем через временный + rename (атомарность: обрезок dek.bin
         * = потеря Vault, это единственный ключ). Result: порча dek.bin ->
         * failure([DekError.CORRUPTED]), не исключение-непойманка. */
        internal fun loadOrCreateDek(dekFile: File, provider: DekProvider): Result<ByteArray> {
            if (dekFile.isFile) {
                val unwrapped = provider.unwrap(dekFile.readBytes())
                if (unwrapped.isFailure) return unwrapped
                val dek = unwrapped.getOrThrow()
                return if (dek.size == 32) {
                    Result.success(dek)
                } else {
                    Result.failure(DekError.INVALID_LENGTH)
                }
            }
            val dek = ByteArray(32).also { SecureRandom().nextBytes(it) }
            val wrapped = provider.wrap(dek).getOrThrow()
            val tmp = File(dekFile.parentFile, dekFile.name + ".tmp")
            tmp.writeBytes(wrapped)
            check(tmp.renameTo(dekFile)) { "cannot rename ${tmp.name} -> ${dekFile.name}" }
            return Result.success(dek)
        }

        /** Миграции до [VaultSchema.SCHEMA_VERSION]. meta-таблицы на свежей
         * БД ещё нет — читаем schema_version лениво (try/catch = версия 0). */
        private fun migrate(db: SQLiteDatabase) {
            val current = readSchemaVersion(db)
            VaultSchema.migrationStatements(current).forEach { db.execSQL(it) }
        }

        private fun readSchemaVersion(db: SQLiteDatabase): Int {
            // Свежая БД: таблицы meta ещё нет — rawQuery бросит SQLiteException,
            // это НЕ ошибка, а признак версии 0 (первый запуск).
            val cursor = try {
                db.rawQuery(
                    "SELECT value FROM ${VaultSchema.TABLE_META} WHERE key='schema_version'",
                    null,
                )
            } catch (_: Exception) {
                return 0
            } ?: return 0
            return cursor.use {
                if (it.moveToFirst()) it.getString(0).toIntOrNull() ?: 0 else 0
            }
        }
    }

    /** CBOR NodeState (этап B, экспорт ноды) — в chat_state по ключу
     * 'node_state'. Возвращает false при закрытой БД. */
    @Synchronized
    fun saveNodeState(bytes: ByteArray): Boolean {
        if (!db.isOpen) return false
        db.execSQL(
            "INSERT OR REPLACE INTO ${VaultSchema.TABLE_CHAT_STATE}(key, value) VALUES(?, ?)",
            arrayOf(KEY_NODE_STATE, bytes),
        )
        return true
    }

    @Synchronized
    fun loadNodeState(): ByteArray? {
        if (!db.isOpen) return null
        val cursor = db.rawQuery(
            "SELECT value FROM ${VaultSchema.TABLE_CHAT_STATE} WHERE key=?",
            arrayOf(KEY_NODE_STATE),
        ) ?: return null
        return cursor.use {
            if (it.moveToFirst()) it.getBlob(0) else null
        }
    }

    /** Контакт: upsert имени и уровня доверия (§3.1/§3.2). verified_at
     *  ставится ТОЛЬКО при переходе в VERIFIED; VERIFICATION_FAILED его не
     *  трогает (момент последней успешной сверки сохраняется). */
    @Synchronized
    fun upsertContact(pubkey: ByteArray, name: String, trustLevel: TrustLevel) {
        db.execSQL(
            """
            INSERT INTO ${VaultSchema.TABLE_CONTACTS}(pubkey, name, trust_level, verified_at)
            VALUES(?, ?, ?, ${if (trustLevel == TrustLevel.VERIFIED) "CAST(strftime('%s','now') AS INTEGER)" else "NULL"})
            ON CONFLICT(pubkey) DO UPDATE SET
                name = excluded.name,
                trust_level = excluded.trust_level,
                verified_at = CASE
                    WHEN excluded.trust_level = 'verified' AND ${VaultSchema.TABLE_CONTACTS}.verified_at IS NULL
                    THEN CAST(strftime('%s','now') AS INTEGER)
                    ELSE ${VaultSchema.TABLE_CONTACTS}.verified_at
                END
            """.trimIndent(),
            arrayOf(pubkey, name, trustLevel.toDbValue()),
        )
    }

    /** BLOB-литерал для инлайна в SQL. sqlcipher-android 4.17 не биндит
     *  ByteArray из selectionArgs как BLOB (WHERE pubkey=? молча не матчит
     *  записанный BLOB) — поэтому BLOB-ключи в rawQuery инлайнятся hex-строкой.
     *  Pin-поведение: BlobBindingRegressionTest. */
    private fun blobLiteral(b: ByteArray): String =
        b.joinToString(separator = "", prefix = "x'", postfix = "'") { "%02x".format(it) }

    @Synchronized
    fun contactTrustLevel(pubkey: ByteArray): TrustLevel? {
        val cursor = db.rawQuery(
            "SELECT trust_level FROM ${VaultSchema.TABLE_CONTACTS} WHERE pubkey=${blobLiteral(pubkey)}",
            null,
        ) ?: return null
        return cursor.use {
            if (it.moveToFirst()) TrustLevel.fromDbValue(it.getString(0)) else null
        }
    }

    /** Контакт из Vault (загрузка при старте, MUSH-B-SPEC §1.3/§3.1). */
    data class StoredContact(
        val pubkey: ByteArray,
        val name: String,
        val trust: TrustLevel,
    )

    /** Re-handshake merge (MUSH-B-SPEC §3.2): имя — из свежего handshake,
     *  trust_level и verified_at СОХРАНЯЮТСЯ (verification не сбрасывается
     *  при повторном подключении). SQL — [VaultSchema.insertContactMerge]. */
    @Synchronized
    fun addContactIfAbsent(pubkey: ByteArray, name: String) {
        db.execSQL(
            VaultSchema.insertContactMerge(),
            arrayOf(pubkey, name, TrustLevel.UNVERIFIED.toDbValue()),
        )
    }

    /** Все контакты (для восстановления списка после рестарта). */
    @Synchronized
    fun contacts(): List<StoredContact> {
        val out = mutableListOf<StoredContact>()
        val cursor = db.rawQuery(
            "SELECT pubkey, name, trust_level FROM ${VaultSchema.TABLE_CONTACTS} ORDER BY name",
            null,
        ) ?: return out
        cursor.use {
            while (it.moveToNext()) {
                out.add(
                    StoredContact(
                        pubkey = it.getBlob(0),
                        name = it.getString(1),
                        trust = TrustLevel.fromDbValue(it.getString(2)),
                    ),
                )
            }
        }
        return out
    }

    @Synchronized
    fun addMessage(groupId: ByteArray, sender: ByteArray, lamport: Long, text: String, createdAt: Long) {
        db.execSQL(
            "INSERT INTO ${VaultSchema.TABLE_MESSAGES}(group_id, sender, lamport, text, created_at) VALUES(?, ?, ?, ?, ?)",
            arrayOf(groupId, sender, lamport, text, createdAt),
        )
    }

    @Synchronized
    fun messagesFor(groupId: ByteArray): List<VaultMessage> {
        val out = mutableListOf<VaultMessage>()
        val cursor = db.rawQuery(
            "SELECT id, group_id, sender, lamport, text, created_at FROM ${VaultSchema.TABLE_MESSAGES} WHERE group_id=${blobLiteral(groupId)} ORDER BY lamport",
            null,
        ) ?: return out
        cursor.use {
            while (it.moveToNext()) {
                out.add(
                    VaultMessage(
                        id = it.getLong(0),
                        groupId = it.getBlob(1),
                        sender = it.getBlob(2),
                        lamport = it.getLong(3),
                        text = it.getString(4),
                        createdAt = it.getLong(5),
                    ),
                )
            }
        }
        return out
    }

    @Synchronized
    fun close() {
        if (db.isOpen) db.close()
    }
}
