package app.mush.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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

/** Onboarding (§8.1) — день 6 (MUSH-B-SPEC §2): «твой идентикон» из pubkey
 *  (Rust-рендер через IdenticonView), identity уже создана на Splash
 *  (флаг 4): отмена/возврат сюда показывает ТОТ ЖЕ идентикон. Имя в hash
 *  не входит — идентикон зависит только от pubkey, смена имени его не меняет.
 *  Onboarding только пишет display name и дёргает StartNetworking. */
@Composable
fun OnboardingScreen(onDone: (name: String) -> Unit) {
    var name by remember { mutableStateOf("") }
    val state by MushStore.state.collectAsState()

    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(20.dp),
        modifier = Modifier
            .fillMaxSize()
            .padding(32.dp),
    ) {
        // Идентикон уже на первом показе — Splash гарантирует myPubkey.
        IdenticonView(pubkeyB64 = state.myPubkey, size = 96.dp)
        Text("Это вы", style = MaterialTheme.typography.titleMedium)
        Text("Как вас называть?", style = MaterialTheme.typography.titleLarge)
        OutlinedTextField(
            value = name,
            onValueChange = { name = it.take(24) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
            placeholder = { Text("Имя") },
        )
        Button(
            onClick = { onDone(name.trim().ifEmpty { "anon" }) },
            enabled = name.isNotBlank() && state.myPubkey.isNotEmpty(),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Далее")
        }
    }
}
