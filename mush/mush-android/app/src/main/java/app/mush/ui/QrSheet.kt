package app.mush.ui

import android.graphics.Bitmap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions

/** QR-модалки (§8.1): НЕ отдельный route — ModalBottomSheet поверх экрана.
 *
 *  Показ: ZXing BarcodeEncoder, payload — полная invite-строка (§5.0,
 *  Invite::render целиком: pubkey, nonce, addr, group).
 *  Скан: ScanContract (zxing-android-embedged), результат — invite ->
 *  NodeCommand.Join. Камера — runtime permission, запрашивается системой. */

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun QrShowSheet(invite: String, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
            modifier = Modifier
                .fillMaxWidth()
                .padding(24.dp),
        ) {
            Text("Ваш QR для контакта", style = MaterialTheme.typography.titleMedium)
            val bitmap = rememberQrBitmap(invite)
            if (bitmap != null) {
                Image(
                    bitmap = bitmap.asImageBitmap(),
                    contentDescription = "Invite QR",
                    modifier = Modifier.size(280.dp),
                )
            } else {
                Text("QR не сгенерировался", style = MaterialTheme.typography.bodySmall)
            }
            Text(
                invite.take(48) + "…",
                style = MaterialTheme.typography.bodySmall,
            )
            // Wi-Fi-креденшелы AP: клиент подключается к сети ДО скана.
            // TODO(post-A'): перенести в Wi-Fi-QR / WifiNetworkSpecifier.
            val creds = androidx.compose.runtime.remember {
                app.mush.NetworkManager.hotspotCredentials()
            }
            if (creds != null) {
                Text(
                    "Wi-Fi: ${creds.first}\nПароль: ${creds.second}",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            Button(onClick = onDismiss) { Text("Закрыть") }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun QrScanSheet(onInvite: (String) -> Unit, onDismiss: () -> Unit) {
    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        if (result.contents != null) {
            onInvite(result.contents)
        } else {
            onDismiss()
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
            modifier = Modifier
                .fillMaxWidth()
                .padding(24.dp),
        ) {
            Text("Отсканируйте QR собеседника", style = MaterialTheme.typography.titleMedium)
            Button(onClick = { scanLauncher.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)) }) {
                Text("Сканировать")
            }
            Text(
                "Нужна камера. QR несёт полную invite-строку (§5.0).",
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

@Composable
private fun rememberQrBitmap(content: String): Bitmap? =
    androidx.compose.runtime.remember(content) {
        runCatching {
            com.google.zxing.MultiFormatWriter().encode(
                content,
                BarcodeFormat.QR_CODE,
                1024,
                1024,
            ).let { matrix ->
                val w = matrix.width
                val h = matrix.height
                val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.RGB_565)
                for (x in 0 until w) {
                    for (y in 0 until h) {
                        bmp.setPixel(x, y, if (matrix.get(x, y)) 0xFF000000.toInt() else 0xFFFFFFFF.toInt())
                    }
                }
                bmp
            }
        }.getOrNull()
    }
