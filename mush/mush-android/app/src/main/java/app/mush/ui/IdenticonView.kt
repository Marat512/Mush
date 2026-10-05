package app.mush.ui

import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import app.mush.Native
import coil.compose.AsyncImage
import coil.decode.SvgDecoder
import coil.request.ImageRequest

/** Идентикон pubkey (MUSH-B-SPEC §2). Рендеринг — Coil + SvgDecoder (НЕ
 * WebView, НЕ Compose Canvas): SVG генерирует Rust (`Native.identiconSvg`),
 * Kotlin не рендерит идентикон из bytes для UI — Rust единственный источник
 * правды. Кеш — стандартный Coil (memory), свой не городим.
 *
 * SVG получаем через produceState на Default-диспетчере: JNI-вызов
 * блокирующий, 100+ идентиконов в LazyColumn не должны фризить скролл.
 * LazyColumn-списки обязаны использовать key (pubkey) — в ContactsScreen
 * уже `key = { it.pubkey }`, не дублируем. */
@Composable
fun IdenticonView(pubkeyB64: String, size: Dp, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val svg by produceState<String?>(initialValue = null, pubkeyB64) {
        // Дублирующие запросы с тем же pubkey дедуплицируются produceState
        // (перезапуск только при смене pubkeyB64).
        value = runCatching {
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) {
                Native.identiconSvg(pubkeyB64)
            }
        }.getOrNull()
    }
    val model = svg?.let {
        ImageRequest.Builder(context)
            .data(it.toByteArray())
            // Только SvgDecoder: байты — это SVG-строка из Rust.
            .decoderFactory(SvgDecoder.Factory())
            .build()
    }
    AsyncImage(
        model = model,
        contentDescription = "identicon",
        modifier = modifier.size(size),
    )
}
