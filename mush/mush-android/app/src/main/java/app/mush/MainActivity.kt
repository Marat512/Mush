package app.mush

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** Экран-пробник Milestone 2: нода живёт в [MushService] (foreground), UI
 * только наблюдает [NodeState]. Кнопка: runtime permissions (§7) →
 * startForegroundService. Отказ от hotspot-permission не блокирует — нода
 * стартует в LAN-режиме (критерий #3 §10). */
class MainActivity : ComponentActivity() {
    private val permissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { results ->
            // Отказ — не блокер: LAN fallback. Нода всё равно стартует.
            val hotspotGranted =
                Permissions.hotspotPermissions().all { results[it] == true }
            if (!hotspotGranted) {
                NodeState.event("hotspot permission denied — LAN mode")
            }
            startService()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    NodeProbe(onStart = { startNodeWithPermissions() })
                }
            }
        }
    }

    private fun startNodeWithPermissions() {
        if (!Permissions.allGranted(this, Permissions.requestSet())) {
            permissionLauncher.launch(Permissions.requestSet())
            return
        }
        startService()
    }

    private fun startService() {
        val intent = Intent(this, MushService::class.java)
        startForegroundService(intent)
    }
}

@Composable
fun NodeProbe(onStart: () -> Unit) {
    val state by NodeState.state.collectAsState()

    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.padding(24.dp),
        ) {
            Text(text = state.status, style = MaterialTheme.typography.titleMedium)
            if (state.pubkey.isNotEmpty()) {
                Text(
                    text = "pubkey: ${state.pubkey.take(16)}…",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            if (state.invite.isNotEmpty()) {
                Text(
                    text = "invite: ${state.invite.take(32)}…",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Button(enabled = !state.running, onClick = onStart) {
                Text("Start node")
            }
            state.events.forEach {
                Text(text = it, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
