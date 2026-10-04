package app.mush.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** Chats (§8.1): список групп. В A' рабочая группа одна — показываем её
 *  карточкой с последним сообщением. FAB «+» → QR-скан (join через §5.0). */
@Composable
fun ChatsScreen(
    viewModel: MushViewModel,
    onOpenChat: () -> Unit,
    onScanQr: () -> Unit,
) {
    val state by viewModel.state.collectAsState()
    val last = state.messages.lastOrNull()

    Box(modifier = Modifier.fillMaxSize()) {
        Column(modifier = Modifier.fillMaxSize()) {
            StatusLine(state)
            LazyColumn(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(horizontal = 16.dp),
            ) {
                items(1) {
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 8.dp)
                            .clickable(onClick = onOpenChat),
                    ) {
                        Row(
                            modifier = Modifier.padding(16.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column {
                                Text("Рабочая группа", style = MaterialTheme.typography.titleMedium)
                                Text(
                                    text = last?.let { "${it.senderName}: ${it.text}" }
                                        ?: "Нет сообщений. Нажмите + и отсканируйте QR собеседника.",
                                    style = MaterialTheme.typography.bodySmall,
                                    maxLines = 1,
                                )
                            }
                        }
                    }
                }
            }
        }
        ExtendedFloatingActionButton(
            onClick = onScanQr,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .padding(16.dp),
        ) {
            Text("＋ QR")
        }
    }
}

@Composable
fun StatusLine(state: UiState) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(state.statusLine, style = MaterialTheme.typography.bodySmall)
        Text(
            when (state.connection) {
                is ConnectionState.Hotspot -> "📡 AP"
                is ConnectionState.Lan -> "🌐 LAN"
                ConnectionState.None -> "⚫"
            },
            style = MaterialTheme.typography.bodySmall,
        )
    }
}
