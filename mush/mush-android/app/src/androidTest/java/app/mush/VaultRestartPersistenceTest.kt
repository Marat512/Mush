package app.mush

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Этап B день 8 (MUSH-B-SPEC §7): restart-persistence Vault на эмуляторе.
 *
 *  JVM-тесты (VaultLogicTest) намеренно без SQLCipher-native — здесь, на
 *  устройстве, закрываем интеграцию persistence-слоя: trust-уровень и
 *  история переживают close() + переоткрытие того же файла БД (симуляция
 *  рестарта хранилища; настоящий force-stop процесса — hardware-check.sh §6,
 *  изнутри тест-процесса он недостижим и orchestrator ради него не тянем). */
@RunWith(AndroidJUnit4::class)
class VaultRestartPersistenceTest {

    private lateinit var context: android.content.Context

    @Before
    fun wipeVault() {
        context = ApplicationProvider.getApplicationContext()
        // Чистый Vault на каждый тест: дни 4–7 могли оставить данные
        // ручных прогонов на эмуляторе.
        File(context.filesDir, "vault").deleteRecursively()
    }

    private fun key(seed: Int): ByteArray = ByteArray(32) { (it + seed).toByte() }

    /** Ядро сценария: экземпляр №1 пишет, закрывается; экземпляр №2 на той
     *  же БД читает. [trust] — уровень доверия, [contactName] — имя. */
    private fun trustSurvivesReopen(trust: TrustLevel, contactName: String) {
        val pubkey = key(1)
        val groupId = key(100)

        val v1 = VaultManager.open(context)
        try {
            v1.upsertContact(pubkey, contactName, trust)
            // addMessage — часть сценария дня 8 (история пишется в тот же
            // commit-цикл Vault). Чтение истории после reopen — в v2 ниже
            // (связку BLOB-ключей pin'ит BlobBindingRegressionTest).
            v1.addMessage(groupId, pubkey, 1, "первое", 1_700_000_000_000L)
            v1.addMessage(groupId, pubkey, 2, "второе", 1_700_000_001_000L)
        } finally {
            v1.close()
        }

        val v2 = VaultManager.open(context)
        try {
            // Чтение — через contacts(): это production-путь восстановления
            // списка (MushService.restoreContacts).
            val contacts = v2.contacts()
            assertEquals("контакт пережил close()+reopen", 1, contacts.size)
            assertEquals(contactName, contacts[0].name)
            assertEquals(trust, contacts[0].trust)
            assertArrayEquals(pubkey, contacts[0].pubkey)
            assertEquals(
                "история пережила close()+reopen",
                listOf("первое", "второе"),
                v2.messagesFor(groupId).map { it.text },
            )
        } finally {
            v2.close()
        }
    }

    @Test
    fun verifiedTrustSurvivesVaultReopen() {
        trustSurvivesReopen(TrustLevel.VERIFIED, "Алиса Вериф")
    }

    @Test
    fun verificationFailedTrustSurvivesVaultReopen() {
        trustSurvivesReopen(TrustLevel.VERIFICATION_FAILED, "Боб Плох")
    }
}
