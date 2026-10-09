package app.mush

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Регрессия: ByteArray-аргумент WHERE-колонки BLOB должен матчить записанный
 *  BLOB. В sqlcipher-android 4.17 rawQuery(selectionArgs) не биндит ByteArray
 *  как BLOB — contactTrustLevel() возвращала null, messagesFor() — пусто
 *  (обе мертвы в main, но messagesFor нужна для истории чатов; тихий пустой
 *  ответ — худший вид бага). Фикс: BLOB-аргумент инлайнится hex-литералом.
 *  Тест пинит поведение: до фикса красный, после — зелёный. */
@RunWith(AndroidJUnit4::class)
class BlobBindingRegressionTest {

    private lateinit var context: android.content.Context

    @Before
    fun wipeVault() {
        context = ApplicationProvider.getApplicationContext()
        File(context.filesDir, "vault").deleteRecursively()
    }

    @Test
    fun byteArrayWhereArgMatchesStoredBlob() {
        val pubkey = ByteArray(32) { it.toByte() }
        val groupId = ByteArray(32) { (it + 100).toByte() }

        val v1 = VaultManager.open(context)
        try {
            v1.upsertContact(pubkey, "Алиса", TrustLevel.VERIFIED)
            v1.addMessage(groupId, pubkey, 1, "первое", 1_700_000_000_000L)
            v1.addMessage(groupId, pubkey, 2, "второе", 1_700_000_001_000L)
        } finally {
            v1.close()
        }

        val v2 = VaultManager.open(context)
        try {
            assertEquals(
                "contactTrustLevel должна найти контакт по BLOB-ключу",
                TrustLevel.VERIFIED,
                v2.contactTrustLevel(pubkey),
            )
            assertEquals(
                "messagesFor должна вернуть историю группы",
                listOf("первое", "второе"),
                v2.messagesFor(groupId).map { it.text },
            )
        } finally {
            v2.close()
        }
    }
}
