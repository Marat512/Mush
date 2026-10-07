package app.mush.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** Contacts (§8.1) + дни 6–7 (MUSH-B-SPEC §2, §3.1): идентикон у каждой
 *  строки, галочка Verified, «Сверить» — modal safety number, long-press —
 *  debug-пометка Verified (та же запись Vault, что и сверка). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ContactsScreen(viewModel: MushViewModel, onShowQr: () -> Unit) {
    val state by viewModel.state.collectAsState()
    var verifyTarget by remember { mutableStateOf<Contact?>(null) }

    Column(modifier = Modifier.fillMaxSize()) {
        StatusLine(state)
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                // Мой идентикон (маленький, AppBar-стиль).
                IdenticonView(pubkeyB64 = state.myPubkey, size = 32.dp)
                Text(
                    "Мой ключ: ${state.myPubkey.take(12)}…",
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(start = 8.dp),
                )
            }
            Button(onClick = onShowQr) { Text("Показать QR") }
        }
        if (state.contacts.isEmpty()) {
            Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(
                    "Контактов нет.\nПокажите QR собеседнику или отсканируйте его.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        } else {
            LazyColumn(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(horizontal = 16.dp),
            ) {
                // key = pubkey: уникален, produceState в IdenticonView не
                // перезапускается при перекомпозиции скролла.
                items(state.contacts, key = { it.pubkey }) { contact ->
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 4.dp)
                            .combinedClickable(
                                onClick = { verifyTarget = contact },
                                // Debug long-press: верификация без сверки
                                // (plumbing-проверка §3.1). Полный flow —
                                // tap -> SafetyNumberSheet.
                                onLongClick = {
                                    viewModel.send(NodeCommand.SetTrust(contact.pubkey, UiTrust.VERIFIED))
                                },
                            ),
                    ) {
                        Row(
                            modifier = Modifier.padding(12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            IdenticonView(
                                pubkeyB64 = contact.pubkey,
                                size = 40.dp,
                                modifier = Modifier.padding(end = 12.dp),
                            )
                            Text(
                                if (contact.connected) "🟢" else "⚪",
                                modifier = Modifier.padding(end = 8.dp),
                            )
                            Column(modifier = Modifier.weight(1f)) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Text(
                                        contact.name,
                                        style = MaterialTheme.typography.titleSmall,
                                    )
                                    // §3.2: verified — галочка, verification_failed
                                    // — красный индикатор (отправка не блокируется).
                                    when (contact.trust) {
                                        UiTrust.VERIFIED -> Text(
                                            " ✓ verified",
                                            style = MaterialTheme.typography.bodySmall,
                                            color = MaterialTheme.colorScheme.primary,
                                            modifier = Modifier.padding(start = 4.dp),
                                        )
                                        UiTrust.FAILED -> Text(
                                            " ✗ сверка не совпала",
                                            style = MaterialTheme.typography.bodySmall,
                                            color = MaterialTheme.colorScheme.error,
                                            modifier = Modifier.padding(start = 4.dp),
                                        )
                                        UiTrust.UNVERIFIED -> Unit
                                    }
                                }
                                Text(
                                    contact.pubkey.take(20) + "…",
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                            Button(onClick = { verifyTarget = contact }) {
                                Text("Сверить")
                            }
                        }
                    }
                }
            }
        }
    }

    // День 7 (§3.1): сверка safety number с выбранным контактом.
    verifyTarget?.let { contact ->
        SafetyNumberSheet(
            contact = contact,
            myPubkeyB64 = state.myPubkey,
            onTrust = { trust -> viewModel.send(NodeCommand.SetTrust(contact.pubkey, trust)) },
            onDismiss = { verifyTarget = null },
        )
    }
}
