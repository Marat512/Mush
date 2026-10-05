package app.mush.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

/** Splash (§8.1). Флаг 4 (день 6): identity обязана существовать К ОНЦУ
 *  Splash — ждём myPubkey в MushStore (сервис стартует из MainActivity
 *  сразу, initNode идёт в фоне), а не фиксированный таймаут. Onboarding
 *  после этого показывает ТОТ ЖЕ идентикон при любом возврате/отмене. */
@Composable
fun SplashScreen(onTimeout: () -> Unit) {
    val state by MushStore.state.collectAsState()
    LaunchedEffect(Unit) {
        // Страховка от вечного спиннера: initNode сам переведёт фазу в Error
        // при сбое — тогда тоже уходим (Onboarding покажет статус-строку).
        val deadline = System.currentTimeMillis() + 20_000
        while (state.myPubkey.isEmpty() && state.phase != NodePhase.Error &&
            System.currentTimeMillis() < deadline
        ) {
            delay(100)
        }
        onTimeout()
    }
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            Text("🍄", style = MaterialTheme.typography.displayLarge)
            CircularProgressIndicator(modifier = Modifier.size(36.dp))
            Text("Создаём ключ…", style = MaterialTheme.typography.bodyMedium)
        }
    }
}
