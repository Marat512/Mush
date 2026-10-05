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

/** Уровень доверия контакта (MUSH-B-SPEC §3.1: Verified после сверки safety
 * number; отправка без Verified не блокируется, но баннер остаётся). */
enum class TrustLevel {
    UNVERIFIED,
    VERIFIED,
    ;

    fun toDbValue(): String = name.lowercase()

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
         * schema_version=1. [dekProvider] — временно plaintext, см. класс. */
        @JvmStatic
        fun open(context: Context, dekProvider: DekProvider = PlaintextDekProvider()): VaultManager {
            // sqlcipher-android 4.17: классы AAR не грузят нативную либу сами
            // (нет loadLibs в API) — грузим явно. Только здесь, в Android-пути:
            // companion-init грузил бы либу и в JVM-тестах (UnsatisfiedLinkError).
            System.loadLibrary("sqlcipher")
            val dir = File(context.filesDir, "vault").apply { mkdirs() }
            val dek = loadOrCreateDek(File(dir, "dek.bin"), dekProvider)
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
         * = потеря Vault, это единственный ключ). */
        internal fun loadOrCreateDek(dekFile: File, provider: DekProvider): ByteArray {
            if (dekFile.isFile) {
                val dek = provider.unwrap(dekFile.readBytes())
                require(dek.size == 32) { "dek.bin: ожидалось 32 байта, got ${dek.size}" }
                return dek
            }
            val dek = ByteArray(32).also { SecureRandom().nextBytes(it) }
            val tmp = File(dekFile.parentFile, dekFile.name + ".tmp")
            tmp.writeBytes(provider.wrap(dek))
            check(tmp.renameTo(dekFile)) { "cannot rename ${tmp.name} -> ${dekFile.name}" }
            return dek
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

    /** Контакт: upsert имени и уровня доверия (§3.1). */
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

    @Synchronized
    fun contactTrustLevel(pubkey: ByteArray): TrustLevel? {
        val cursor = db.rawQuery(
            "SELECT trust_level FROM ${VaultSchema.TABLE_CONTACTS} WHERE pubkey=?",
            arrayOf(pubkey),
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

    /** Добавить контакт, если его ещё нет (handshake не затирает имя,
     *  заданное пользователем, и не сбрасывает trust). */
    @Synchronized
    fun addContactIfAbsent(pubkey: ByteArray, name: String) {
        db.execSQL(
            "INSERT OR IGNORE INTO ${VaultSchema.TABLE_CONTACTS}(pubkey, name, trust_level) VALUES(?, ?, ?)",
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
            "SELECT id, group_id, sender, lamport, text, created_at FROM ${VaultSchema.TABLE_MESSAGES} WHERE group_id=? ORDER BY lamport",
            arrayOf(groupId),
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
