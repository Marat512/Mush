package app.mush

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** JVM-тесты чистой логики Vault (MUSH-B-SPEC §5(4), §8): без SQLCipher-native
 * и без Robolectric — интеграция с БД проверяется на эмуляторе (допустимо по
 * спеке, если Robolectric + нативные либы не взлетают быстро). */
class VaultLogicTest {

    /** Фейк-провайдер с обратимой обёрткой (XOR с фиксированным pad):
     * roundtrip должен вернуть исходный DEK, при этом записанное в файл
     * должно отличаться от сырого DEK (иначе обёртка — фикция). */
    private class XorDekProvider : DekProvider {
        val pad = ByteArray(32) { (it * 7 + 3).toByte() }
        override fun wrap(dek: ByteArray): Result<ByteArray> =
            Result.success(
                dek.mapIndexed { i, b -> (b.toInt() xor pad[i % pad.size].toInt()).toByte() }.toByteArray(),
            )

        override fun unwrap(wrapped: ByteArray): Result<ByteArray> =
            wrap(wrapped) // XOR — самообратная операция с тем же pad
    }

    @Test
    fun freshDatabaseMigrationEndsAtSchemaVersion1() {
        val steps = VaultSchema.migrationStatements(0)
        // Все четыре таблицы создаются, версия фиксируется последним шагом.
        assertTrue(steps.any { it.contains("CREATE TABLE IF NOT EXISTS messages") })
        assertTrue(steps.any { it.contains("CREATE TABLE IF NOT EXISTS contacts") })
        assertTrue(steps.any { it.contains("CREATE TABLE IF NOT EXISTS chat_state") })
        assertTrue(steps.any { it.contains("CREATE TABLE IF NOT EXISTS meta") })
        assertEquals(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', '1')",
            steps.last(),
        )
    }

    @Test
    fun currentSchemaRequiresNoMigration() {
        assertTrue(VaultSchema.migrationStatements(VaultSchema.SCHEMA_VERSION).isEmpty())
    }

    @Test(expected = IllegalStateException::class)
    fun newerSchemaIsRejected() {
        VaultSchema.migrationStatements(VaultSchema.SCHEMA_VERSION + 1)
    }

    @Test
    fun dekRoundtripThroughFakeProvider() {
        val dir = kotlin.io.path.createTempDirectory("vault-test").toFile().apply { deleteOnExit() }
        val dekFile = File(dir, "dek.bin")
        val provider = XorDekProvider()

        val dek1 = VaultManager.loadOrCreateDek(dekFile, provider).getOrThrow()
        assertEquals(32, dek1.size)
        // Записанное — обёрнутое (отличается от сырого DEK).
        assertFalse(dekFile.readBytes().contentEquals(dek1))

        // Повторное открытие читает тот же DEK через unwrap.
        val dek2 = VaultManager.loadOrCreateDek(dekFile, provider).getOrThrow()
        assertArrayEquals(dek1, dek2)
    }

    @Test
    fun corruptedDekFileFailsWithCorrupted() {
        val dir = kotlin.io.path.createTempDirectory("vault-test").toFile().apply { deleteOnExit() }
        val dekFile = File(dir, "dek.bin")
        dekFile.writeBytes(ByteArray(16)) // не 32 — порча
        val result = VaultManager.loadOrCreateDek(dekFile, XorDekProvider())
        assertTrue(result.isFailure)
        val err = result.exceptionOrNull()
        assertTrue(
            "ожидали DekError, got $err",
            err is DekError.CORRUPTED || err is DekError.INVALID_LENGTH,
        )
    }

    @Test
    fun plaintextProviderIsIdentityForNow() {
        // Намеренный временный шаг (MUSH-B-SPEC §6): wrap/unwrap — копия.
        // Тест пинит контракт провайдера, не силу обёртки.
        val p = PlaintextDekProvider()
        val dek = ByteArray(32) { it.toByte() }
        assertArrayEquals(dek, p.unwrap(p.wrap(dek).getOrThrow()).getOrThrow())
    }

    @Test
    fun plaintextUnwrapRejectsWrongLength() {
        val p = PlaintextDekProvider()
        val bad = p.unwrap(ByteArray(16))
        assertTrue(bad.isFailure)
        assertTrue(bad.exceptionOrNull() is DekError.CORRUPTED)
    }

    @Test
    fun trustLevelMapping() {
        assertEquals(TrustLevel.VERIFIED, TrustLevel.fromDbValue("verified"))
        assertEquals(TrustLevel.UNVERIFIED, TrustLevel.fromDbValue("unverified"))
        // §3.2: verification_failed — добавочное TEXT-значение в v1-колонке
        // (DDL-миграция не нужна, schema_version остаётся 1).
        assertEquals(TrustLevel.VERIFICATION_FAILED, TrustLevel.fromDbValue("verification_failed"))
        // DEFAULT 'unverified' в схеме; неизвестное значение — UNVERIFIED, не crash.
        assertEquals(TrustLevel.UNVERIFIED, TrustLevel.fromDbValue("corrupted"))
        assertEquals("verified", TrustLevel.VERIFIED.toDbValue())
        assertEquals("unverified", TrustLevel.UNVERIFIED.toDbValue())
        assertEquals("verification_failed", TrustLevel.VERIFICATION_FAILED.toDbValue())
    }

    /** §3.2 re-handshake merge: conflict-ветка обновляет имя, но НЕ
     *  trust_level/verified_at (verification не сбрасывается). Единый
     *  источник SQL — VaultSchema.insertContactMerge(). */
    @Test
    fun rehandshakeMergeUpdatesNameButKeepsTrust() {
        val sql = VaultSchema.insertContactMerge()
        assertTrue("conflict-ветка должна обновлять имя", sql.contains("DO UPDATE SET name = excluded.name"))
        val conflictBranch = sql.substringAfter("ON CONFLICT(pubkey) DO UPDATE SET")
        assertFalse("re-handshake не должен трогать trust_level", conflictBranch.contains("trust_level"))
        assertFalse(conflictBranch.contains("verified_at"))
    }
}
