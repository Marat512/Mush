package app.mush

import android.content.Context
import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import app.mush.ui.MushStore
import app.mush.ui.UiTrust
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.waitUntilAtLeastOneExists
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Этап B день 8 (MUSH-B-SPEC §7): cold-start путь приложения.
 *
 *  Данные кладём в Vault ДО запуска MainActivity: сервис при старте
 *  (MushService.initNode) открывает Vault и дочитывает контакты с реальным
 *  trust_level (§3.2 — не дефолт). Assert — на отображаемое состояние
 *  (галочка «✓ verified» / «✗ сверка не совпала» в ContactsScreen).
 *
 *  Холодный старт здесь = close()+launch заново в одном процессе; источник
 *  правды — Vault на диске. Настоящий force-stop процесса изнутри теста
 *  невозможен (тест живёт в том же процессе) — он остаётся за
 *  hardware-check.sh §6, orchestrator ради него не тянем. */
@RunWith(AndroidJUnit4::class)
class ColdStartTrustTest {

    @get:Rule
    val composeRule = createComposeRule()

    private lateinit var context: Context

    @Before
    fun wipeAppState() {
        context = ApplicationProvider.getApplicationContext()
        // Чистое состояние: ручные прогоны дней 4–7 на эмуляторе не должны
        // влиять на ассерты.
        File(context.filesDir, "vault").deleteRecursively()
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear().commit()
    }

    @After
    fun stopService() {
        context.stopService(Intent(context, MushService::class.java))
    }

    @Test
    @androidx.compose.ui.test.ExperimentalTestApi
    fun contactsTrustRenderedFromVaultAfterColdStart() {
        // Имя ДО старта: Splash с именем идёт сразу в chats (не onboarding).
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString(KEY_NAME, "Тестер").commit()

        // Vault ДО активности: verified- и failed-контакты + пара сообщений.
        val verifiedKey = ByteArray(32) { (it + 1).toByte() }
        val failedKey = ByteArray(32) { (it + 51).toByte() }
        val vault = VaultManager.open(context)
        try {
            vault.upsertContact(verifiedKey, "Алиса Вериф", TrustLevel.VERIFIED)
            vault.upsertContact(failedKey, "Боб Плох", TrustLevel.VERIFICATION_FAILED)
            vault.addMessage(ByteArray(32) { 7 }, verifiedKey, 1, "привет", 1_700_000_000_000L)
        } finally {
            vault.close()
        }

        // Холодный старт: активность (и сервис с initNode) с нуля.
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        try {
            // Splash ждёт identity; myPubkey появляется в store только после
            // restoreContacts() — значит контакты с trust уже в MushStore.
            await("identity создана", 20_000) { MushStore.state.value.myPubkey.isNotEmpty() }

            composeRule.waitUntilAtLeastOneExists(hasText("Contacts"), 15_000)
            composeRule.onNodeWithText("Contacts").performClick()

            // Отображаемое состояние доверия — реальный trust из Vault.
            composeRule.waitUntilAtLeastOneExists(hasText("Алиса Вериф"), 15_000)
            composeRule.onNodeWithText("Алиса Вериф").assertIsDisplayed()
            composeRule.onNodeWithText("Боб Плох").assertIsDisplayed()
            composeRule.onNodeWithText("✓ verified", substring = true).assertIsDisplayed()
            composeRule.onNodeWithText("✗ сверка не совпала", substring = true).assertIsDisplayed()

            // Тот же факт — через UiState (источник данных UI).
            val contacts = MushStore.state.value.contacts
            assertEquals(UiTrust.VERIFIED, contacts.first { it.name == "Алиса Вериф" }.trust)
            assertEquals(UiTrust.FAILED, contacts.first { it.name == "Боб Плох" }.trust)
        } finally {
            scenario.close()
        }
    }

    /** Polling-ожидание для не-compose условий (MushStore — обычный
     *  StateFlow, читаем напрямую, без семантики compose). */
    private fun await(what: String, timeoutMs: Long, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (!condition()) {
            if (System.currentTimeMillis() > deadline) {
                throw AssertionError("таймаут ожидания: $what")
            }
            Thread.sleep(100)
        }
    }

    companion object {
        private const val PREFS = "mush_prefs"
        private const val KEY_NAME = "name"
    }
}
