package app.mush

import android.content.Context
import android.content.pm.ApplicationInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import app.mush.ui.MushStore
import java.net.Inet4Address
import java.net.NetworkInterface

/** Hotspot + LAN transport (A'-ANDROID-SPEC §6).
 *
 * IP не хардкодить: Samsung/Xiaomi/Huawei дают разные диапазоны
 * (192.168.43.1 / 192.168.1.1 / 192.168.232.1…) — ищем через NetworkInterface.
 *
 * Перед startLocalOnlyHotspot обязательна [Permissions.hotspotReady]:
 * granted permission + (API < 33) включённый GPS, иначе SecurityException. */
object NetworkManager {
    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null

    /** Тег = "mush": единый logcat-фильтр прогона (§6.3 тест-плана). */
    private const val TAG = "mush"

    val hotspotActive: Boolean get() = reservation != null

    /** SSID + пароль активного AP (клиенту нужны до скана invite-QR).
     *  API 30+: SoftApConfiguration; 26–29: wifiConfiguration (deprecated). */
    fun hotspotCredentials(): Pair<String, String>? {
        val res = reservation ?: return null
        return if (Build.VERSION.SDK_INT >= 30) {
            val cfg = res.softApConfiguration
            cfg.ssid.toString() to (cfg.passphrase ?: "")
        } else {
            @Suppress("DEPRECATION")
            val cfg = res.wifiConfiguration ?: return null
            val ssid = cfg.SSID?.removeSurrounding("\"") ?: return null
            ssid to (cfg.preSharedKey ?: "")
        }
    }

    fun startHotspot(
        context: Context,
        onReady: (ip: String) -> Unit,
        onFailed: (reason: Int) -> Unit,
    ) {
        val (ready, whyNot) = Permissions.hotspotReady(context)
        if (!ready) {
            // Лог — обязателен: hardware-прогон драйвится через adb-logcat
            // (MUSH-HARDWARE-TEST-PLAN §3.3), UI смотреть неудобно.
            Log.w(TAG, "LocalOnlyHotspot blocked: $whyNot")
            MushStore.update { s -> s.copy(statusLine = "hotspot blocked: $whyNot") }
            onFailed(-1)
            return
        }
        val wifi = context.getSystemService(WifiManager::class.java)
        val callback = object : WifiManager.LocalOnlyHotspotCallback() {
            override fun onStarted(res: WifiManager.LocalOnlyHotspotReservation) {
                reservation = res
                // SSID+pass — только debuggable APK: hardware-прогон драйвится
                // скриптами, клиенту нужны креды ДО QR-скана (§3.3). В release
                // пароль в logcat не пишется.
                if (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
                    Log.i(TAG, "hotspot credentials ssid=${hotspotCredentials()?.first} pass=${hotspotCredentials()?.second}")
                }
                // IP AP может появиться с задержкой (Huawei HicomNfManager
                // сам ретраит ~15с). Две фазы: сначала ищем ТОЛЬКО
                // AP-интерфейс (ap0/swlan — HyperOS даёт ap0=10.51.89.x,
                // а station-wlan0 с WAN-подсетью подходит префиксу 'wlan');
                // если AP-iface не появился за 10с — берём любой подходящий.
                Thread {
                    val ip = (1..20).firstNotNullOfOrNull {
                        findApIp(strict = true) ?: run { Thread.sleep(500); null }
                    } ?: (1..10).firstNotNullOfOrNull {
                        findApIp(strict = false) ?: run { Thread.sleep(500); null }
                    }
                    Handler(Looper.getMainLooper()).post {
                        if (ip != null) {
                            Log.i(TAG, "LocalOnlyHotspotCallback::onStarted ip=$ip")
                            onReady(ip)
                        } else {
                            Log.w(TAG, "LocalOnlyHotspotCallback::onStarted but no AP interface IP after 15s")
                            MushStore.update { s -> s.copy(statusLine = "hotspot up but no AP interface IP after 15s") }
                            onFailed(-2)
                        }
                    }
                }.start()
            }

            override fun onFailed(reason: Int) {
                // reason=3 (ERROR_NO_CHANNEL) — vendor rate-limit
                // (HyperOS): тест-план §3.1 — одна попытка, fallback.
                Log.w(TAG, "LocalOnlyHotspotCallback::onFailed reason=$reason")
                MushStore.update { s -> s.copy(statusLine = "hotspot onFailed($reason)") }
                onFailed(reason)
            }

            override fun onStopped() {
                reservation = null
                Log.i(TAG, "LocalOnlyHotspotCallback::onStopped")
                MushStore.update { s -> s.copy(statusLine = "hotspot stopped") }
            }
        }
        wifi.startLocalOnlyHotspot(callback, Handler(Looper.getMainLooper()))
    }

    fun stopHotspot() {
        reservation?.close()
        reservation = null
    }

    /** IPv4 интерфейса точки доступа.
     *  strict=true: только AP-интерфейсы (ap0, swlan0; иначе на
     *  STA+AP-чипах station-wlan0 с WAN-подсетью выигрывает гонку).
     *  strict=false: + wlan0 fallback. */
    fun findApIp(strict: Boolean): String? =
        findIpv4 { name ->
            name.startsWith("ap") || name.startsWith("swlan") ||
                (!strict && name.startsWith("wlan"))
        }

    /** LAN-адрес устройства (режим fallback: оба телефона в одной Wi-Fi сети). */
    fun lanIp(): String? = findIpv4 { name -> name.startsWith("wlan") || name.startsWith("eth") || name.startsWith("ccmni") }

    private fun findIpv4(ifaceMatch: (String) -> Boolean): String? =
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && ifaceMatch(it.name) }
            .flatMap { it.inetAddresses.toList() }
            .firstOrNull { it is Inet4Address && !it.isLoopbackAddress }
            ?.hostAddress
}
