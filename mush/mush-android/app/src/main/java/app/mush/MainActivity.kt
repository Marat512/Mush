package app.mush

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** MS0-internal (A'-ANDROID-SPEC §9): prove the real node bridge works inside
 * the APK — init → pubkey → listener, with Rust events pushed back into the
 * on-screen log. Two-node messaging acceptance lives in the Rust host tests. */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    NodeProbe(activity = this)
                }
            }
        }
    }
}

@Composable
fun NodeProbe(activity: ComponentActivity) {
    var status by remember { mutableStateOf("Node not started") }
    var pubkey by remember { mutableStateOf("") }
    var invite by remember { mutableStateOf("") }
    var handle by remember { mutableStateOf(0L) }
    val events = remember { mutableStateOf(listOf<String>()) }

    val callback = remember {
        object : MushCallback {
            private fun log(line: String) {
                activity.runOnUiThread { events.value = (events.value + line).takeLast(20) }
            }

            override fun onMessage(groupId: String, senderPubkey: String, text: String, ts: Long) =
                log("onMessage: $text")

            override fun onSensorAlert(sensor: String, targetPubkey: String, reason: String) =
                log("onSensorAlert[$sensor]: $reason")

            override fun onHandshakeComplete(peerPubkey: String) =
                log("onHandshakeComplete: $peerPubkey")

            override fun onError(code: Int, message: String) =
                log("onError($code): $message")
        }
    }

    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.padding(24.dp),
        ) {
            Text(text = status, style = MaterialTheme.typography.titleMedium)
            if (pubkey.isNotEmpty()) {
                Text(text = "pubkey: ${pubkey.take(16)}…", style = MaterialTheme.typography.bodySmall)
            }
            if (invite.isNotEmpty()) {
                Text(text = "invite: ${invite.take(32)}…", style = MaterialTheme.typography.bodySmall)
            }
            Button(
                enabled = handle == 0L,
                onClick = {
                    Thread {
                        try {
                            val h = Native.init(activity.filesDir.absolutePath, callback)
                            val pk = Native.pubkey(h)
                            val port = Native.startListener(h, 0)
                            val inv = Native.invite(h)
                            handle = h
                            pubkey = pk
                            invite = inv
                            status = "Node started on port $port"
                        } catch (t: Throwable) {
                            status = "Error: ${t.message}"
                        }
                    }.start()
                },
            ) {
                Text("Start node")
            }
            events.value.forEach { Text(text = it, style = MaterialTheme.typography.bodySmall) }
        }
    }
}
