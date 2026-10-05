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
        override fun wrap(dek: ByteArray): ByteArray =
            dek.mapIndexed { i, b -> (b.toInt() xor pad[i % pad.size].toInt()).toByte() }.toByteArray()

        override fun unwrap(wrapped: ByteArray): ByteArray =
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

        val dek1 = VaultManager.loadOrCreateDek(dekFile, provider)
        assertEquals(32, dek1.size)
        // Записанное — обёрнутое (отличается от сырого DEK).
        assertFalse(dekFile.readBytes().contentEquals(dek1))

        // Повторное открытие читает тот же DEK через unwrap.
        val dek2 = VaultManager.loadOrCreateDek(dekFile, provider)
        assertArrayEquals(dek1, dek2)
    }

    @Test
    fun plaintextProviderIsIdentityForNow() {
        // Намеренный временный шаг (MUSH-B-SPEC §6): wrap/unwrap — копия.
        // Тест пинит контракт провайдера, не силу обёртки.
        val p = PlaintextDekProvider()
        val dek = ByteArray(32) { it.toByte() }
        assertArrayEquals(dek, p.unwrap(p.wrap(dek)))
    }

    @Test
    fun trustLevelMapping() {
        assertEquals(TrustLevel.VERIFIED, TrustLevel.fromDbValue("verified"))
        assertEquals(TrustLevel.UNVERIFIED, TrustLevel.fromDbValue("unverified"))
        // DEFAULT 'unverified' в схеме; неизвестное значение — UNVERIFIED, не crash.
        assertEquals(TrustLevel.UNVERIFIED, TrustLevel.fromDbValue("corrupted"))
        assertEquals("verified", TrustLevel.VERIFIED.toDbValue())
        assertEquals("unverified", TrustLevel.UNVERIFIED.toDbValue())
    }
}
