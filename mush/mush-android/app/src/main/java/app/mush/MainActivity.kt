package app.mush

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import app.mush.core.MushCore

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    HelloFromRust()
                }
            }
        }
    }
}

/** Milestone 0: Rust code executing inside the APK. Nothing else matters until
 * this string renders on a real device/emulator. */
@Composable
fun HelloFromRust() {
    val greeting = rememberRustHello()
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text(text = greeting, style = MaterialTheme.typography.titleMedium)
    }
}

@Composable
private fun rememberRustHello(): String = androidx.compose.runtime.remember {
    // Runs on first composition; MS0 does not bother with threading.
    runCatching { MushCore.hello() }
        .getOrElse { "Rust bridge error: ${it.message}" }
}
