package app.mush

/** Схема Vault v1 (MUSH-B-SPEC §1.4) — чистые SQL-выражения, отделены от
 * SQLCipher-API, чтобы миграции тестировались на JVM без нативных либ. */
object VaultSchema {
    const val SCHEMA_VERSION = 1

    const val TABLE_MESSAGES = "messages"
    const val TABLE_CONTACTS = "contacts"
    const val TABLE_CHAT_STATE = "chat_state"
    const val TABLE_META = "meta"

    /** CREATE-выражения v1. Идемпотентны (IF NOT EXISTS) — безопасны как для
     * новой БД, так для докатки недостающих таблиц существующей. */
    val CREATE_STATEMENTS: List<String> = listOf(
        """
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            group_id BLOB NOT NULL,
            sender BLOB NOT NULL,
            lamport INTEGER NOT NULL,
            text TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )
        """.trimIndent(),
        "CREATE INDEX IF NOT EXISTS idx_messages_group_lamport ON messages(group_id, lamport)",
        """
        CREATE TABLE IF NOT EXISTS contacts (
            pubkey BLOB PRIMARY KEY,
            name TEXT NOT NULL,
            trust_level TEXT NOT NULL DEFAULT 'unverified',
            verified_at INTEGER,
            last_seen INTEGER
        )
        """.trimIndent(),
        """
        CREATE TABLE IF NOT EXISTS chat_state (
            key TEXT PRIMARY KEY,
            value BLOB NOT NULL
        )
        """.trimIndent(),
        """
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
        """.trimIndent(),
    )

    fun insertSchemaVersion(version: Int): String =
        "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', '$version')"

    /** Выражения для перехода с версии [fromVersion] на текущую.
     * 0 = новая БД. Пустой список — БД уже на актуальной версии. */
    fun migrationStatements(fromVersion: Int): List<String> = when {
        fromVersion < 0 -> throw IllegalArgumentException("bad schema version $fromVersion")
        fromVersion == 0 -> CREATE_STATEMENTS + insertSchemaVersion(SCHEMA_VERSION)
        fromVersion == SCHEMA_VERSION -> emptyList()
        else -> throw IllegalStateException(
            "Vault schema $fromVersion > supported $SCHEMA_VERSION: откат БД не поддерживается",
        )
    }
}
