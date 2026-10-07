package app.mush.ui

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.mush.Native

/** Сверка safety number (MUSH-B-SPEC §3.1/§3.2, дни 6–7). Modal: 60 цифр,
 *  12 групп по 5, monospace + идентиконы обеих сторон (Rust — источник
 *  правды рендеринга); «Скопировать» / «Поделиться» для сверки по третьему
 *  каналу. «Совпадает» -> Verified; «Не совпадает» -> VerificationFailed
 *  (persist Vault, красный индикатор; verified_at не трогаем). Отправка НЕ
 *  блокируется (A'-модель доверия); FAILED снимается только явной повторной
 *  сверкой. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SafetyNumberSheet(
    contact: Contact,
    myPubkeyB64: String,
    onTrust: (UiTrust) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    // JNI — блокирующий вызов: на Default-диспетчере, не на main.
    val number by produceState<String?>(initialValue = null, contact.pubkey) {
        value = runCatching {
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) {
                Native.safetyNumber(myPubkeyB64, contact.pubkey)
            }
        }.getOrNull()
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp)
                .padding(bottom = 32.dp),
        ) {
            Text(
                "Сверка с ${contact.name}",
                style = MaterialTheme.typography.titleLarge,
            )
            // §3.2: FAILED-контакт открывает sheet с пометкой прошлой сверки.
            if (contact.trust == UiTrust.FAILED) {
                Text(
                    "Предыдущая сверка: не совпало",
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            Row(
                horizontalArrangement = Arrangement.spacedBy(24.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    IdenticonView(pubkeyB64 = myPubkeyB64, size = 72.dp)
                    Text("Вы", style = MaterialTheme.typography.bodySmall)
                }
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    IdenticonView(pubkeyB64 = contact.pubkey, size = 72.dp)
                    Text(contact.name, style = MaterialTheme.typography.bodySmall)
                }
            }
            val num = number
            if (num == null) {
                Text("Вычисляю…", style = MaterialTheme.typography.bodyMedium)
            } else {
                // Monospace + selectable: удобно сверять глазами и выделять.
                SelectionContainer {
                    Text(
                        num,
                        style = TextStyle(fontFamily = FontFamily.Monospace),
                        softWrap = true,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    OutlinedButton(onClick = {
                        clipboard.setText(AnnotatedString(num))
                    }) { Text("Скопировать") }
                    OutlinedButton(onClick = {
                        val send = Intent(Intent.ACTION_SEND).apply {
                            type = "text/plain"
                            putExtra(Intent.EXTRA_TEXT, "Mush safety number (${contact.name}):\n$num")
                        }
                        context.startActivity(Intent.createChooser(send, null))
                    }) { Text("Поделиться") }
                }
                when (contact.trust) {
                    UiTrust.VERIFIED -> Text(
                        "✓ Контакт верифицирован",
                        color = MaterialTheme.colorScheme.primary,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    else -> Button(onClick = {
                        onTrust(UiTrust.VERIFIED)
                        onDismiss()
                    }) { Text("Совпадает") }
                }
                if (contact.trust != UiTrust.FAILED) {
                    TextButton(onClick = {
                        // Пользователь явно сказал «не совпадает» — persist
                        // FAILED (не просто предупреждение). Повторная сверка
                        // (которая совпадёт) — единственный путь снять FAILED.
                        onTrust(UiTrust.FAILED)
                        onDismiss()
                    }) {
                        Text("Не совпадает", color = MaterialTheme.colorScheme.error)
                    }
                } else {
                    Text(
                        "⚠ Ключ изменился с прошлого раза. Отправка не " +
                            "блокируется, но контакт остаётся подозрительным, " +
                            "пока вы явно не пересверите.",
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    }
}
