package app.mush.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** Alerts (§8.1): alerts от S1/S7. Действия: «Игнорировать» (снимает с экрана)
 *  и «Заблокировать» (заглушка A' — политики блокировки в этапе E). */
@Composable
fun AlertsScreen(viewModel: MushViewModel) {
    val state by viewModel.state.collectAsState()

    Column(modifier = Modifier.fillMaxSize()) {
        StatusLine(state)
        if (state.alerts.isEmpty()) {
            Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("Алертов нет.", style = MaterialTheme.typography.bodyMedium)
            }
        } else {
            LazyColumn(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(horizontal = 16.dp),
            ) {
                items(state.alerts, key = { it.id }) { alert ->
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 4.dp),
                    ) {
                        Column(modifier = Modifier.padding(12.dp)) {
                            Text(
                                "${alert.sensor}: ${alert.targetPubkey.take(12)}…",
                                style = MaterialTheme.typography.titleSmall,
                            )
                            Text(
                                alert.reason,
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.padding(vertical = 4.dp),
                            )
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Button(onClick = { viewModel.send(NodeCommand.DismissAlert(alert.id)) }) {
                                    Text("Игнорировать")
                                }
                                Button(onClick = { viewModel.send(NodeCommand.DismissAlert(alert.id)) }) {
                                    Text("Заблокировать")
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
