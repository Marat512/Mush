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
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** Chat (§8.1): LazyColumn сообщений + поле ввода. Статус ✓/⏳ — заглушка
 *  (доставка не подтверждается на уровне A'; CRDT-слияние — источник истины). */
@Composable
fun ChatScreen(viewModel: MushViewModel, onBack: () -> Unit) {
    val state by viewModel.state.collectAsState()
    var input by remember { mutableStateOf("") }
    val listState = rememberLazyListState()

    LaunchedEffect(state.messages.size) {
        if (state.messages.isNotEmpty()) listState.animateScrollToItem(state.messages.size - 1)
    }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Button(onClick = onBack) { Text("‹") }
            Text(
                "Рабочая группа",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(start = 12.dp),
            )
        }
        LazyColumn(
            state = listState,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            items(state.messages, key = { it.id }) { msg ->
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = if (msg.mine) Arrangement.End else Arrangement.Start,
                ) {
                    Card {
                        Column(modifier = Modifier.padding(10.dp)) {
                            if (!msg.mine) {
                                Text(msg.senderName, style = MaterialTheme.typography.labelSmall)
                            }
                            Text(msg.text, style = MaterialTheme.typography.bodyMedium)
                        }
                    }
                }
            }
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = input,
                onValueChange = { input = it },
                modifier = Modifier.weight(1f),
                placeholder = { Text("Сообщение…") },
                maxLines = 3,
            )
            Button(
                onClick = {
                    if (input.isNotBlank()) {
                        viewModel.send(NodeCommand.SendMessage(input.trim()))
                        input = ""
                    }
                },
                modifier = Modifier.padding(start = 8.dp),
            ) {
                Text("➤")
            }
        }
        // Debug hooks (§10, критерии 6/7): ручной триггер S1/S7.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 8.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Button(onClick = { viewModel.send(NodeCommand.DebugFlood(50)) }) {
                Text("DBG S1 flood×50")
            }
            Button(onClick = { viewModel.send(NodeCommand.DebugDecoy) }) {
                Text("DBG S7 decoy")
            }
        }
    }
}
