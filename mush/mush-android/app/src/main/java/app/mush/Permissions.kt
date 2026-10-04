package app.mush

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.LocationManager
import android.os.Build
import androidx.core.content.ContextCompat

/** Runtime permissions (A'-ANDROID-SPEC §7 + уточнения по устройствам).
 *
 * Три ветки по API:
 *  - **29–32** (Huawei STK-LX1, Android 10): ACCESS_FINE_LOCATION — legacy-вход
 *    к Wi-Fi/hotspot. На EMUI/Harmony дополнительно нужен включённый GPS:
 *    при выключенном провайдере startLocalOnlyHotspot бросает SecurityException
 *    даже с granted permission.
 *  - **33–34**: NEARBY_WIFI_DEVICES (neverForLocation) + POST_NOTIFICATIONS.
 *  - **35+** (Xiaomi 2312FPCA6G, Android 16): набор тот же, но connectedDevice
 *    foreground service строго проверяется системой — permission'ы должны быть
 *    выданы ДО startForeground, иначе ForegroundServiceStartNotAllowedException.
 *
 * Отказ не блокирует приложение: LAN fallback (§10, критерий #3) работает
 * на чистом INTERNET. */
object Permissions {
    /** Permission'ы, без которых hotspot-режим не поднять. */
    fun hotspotPermissions(): Array<String> = when {
        Build.VERSION.SDK_INT >= 33 -> arrayOf(Manifest.permission.NEARBY_WIFI_DEVICES)
        else -> arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }

    /** Всё, что просим за один заход: hotspot-набор + уведомления (33+). */
    fun requestSet(): Array<String> = when {
        Build.VERSION.SDK_INT >= 33 ->
            arrayOf(
                Manifest.permission.NEARBY_WIFI_DEVICES,
                Manifest.permission.POST_NOTIFICATIONS,
            )
        else -> arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }

    fun allGranted(context: Context, perms: Array<String>): Boolean =
        perms.all {
            ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
        }

    /** GPS-провайдер включён — обязательное условие hotspot на API < 33
     * (иначе SecurityException при granted). На 33+ не требуется. */
    fun isGpsEnabled(context: Context): Boolean =
        Build.VERSION.SDK_INT >= 33 ||
            context.getSystemService(LocationManager::class.java)
                ?.isProviderEnabled(LocationManager.GPS_PROVIDER) == true

    /** Итоговая проверка перед startLocalOnlyHotspot (ветка 2.3). */
    fun hotspotReady(context: Context): Pair<Boolean, String?> {
        if (!allGranted(context, hotspotPermissions())) {
            return false to "hotspot permission denied"
        }
        if (!isGpsEnabled(context)) {
            return false to "GPS provider disabled (required for hotspot on API < 33)"
        }
        return true to null
    }
}
