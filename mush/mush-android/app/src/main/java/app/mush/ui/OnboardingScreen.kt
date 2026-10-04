package app.mush.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** Onboarding (§8.1): поле имени + идентикон (заглушка — цвет по хешу имени;
 *  настоящий идентикон из pubkey — этап B) + «Далее». */
@Composable
fun OnboardingScreen(onDone: (name: String) -> Unit) {
    var name by remember { mutableStateOf("") }

    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(20.dp),
            modifier = Modifier.padding(32.dp),
        ) {
            val identiconColor = remember(name) {
                Color(0xFF000000 or (name.hashCode().toLong() and 0x7FFFFF or 0x404040))
            }
            Box(
                modifier = Modifier
                    .size(96.dp)
                    .clip(CircleShape)
                    .background(identiconColor),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = name.take(1).uppercase().ifEmpty { "?" },
                    style = MaterialTheme.typography.headlineLarge,
                    color = Color.White,
                )
            }
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
                enabled = name.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Далее")
            }
        }
    }
}
