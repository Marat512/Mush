package app.mush

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
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

    val hotspotActive: Boolean get() = reservation != null

    fun startHotspot(
        context: Context,
        onReady: (ip: String) -> Unit,
        onFailed: (reason: Int) -> Unit,
    ) {
        val (ready, whyNot) = Permissions.hotspotReady(context)
        if (!ready) {
            MushStore.update { s -> s.copy(statusLine = "hotspot blocked: $whyNot") }
            onFailed(-1)
            return
        }
        val wifi = context.getSystemService(WifiManager::class.java)
        val callback = object : WifiManager.LocalOnlyHotspotCallback() {
            override fun onStarted(res: WifiManager.LocalOnlyHotspotReservation) {
                reservation = res
                // IP AP может появиться с задержкой (Huawei HicomNfManager
                // сам ретраит ~15с) — поллим интерфейсы, не хардкодим.
                Thread {
                    val ip = (1..30).firstNotNullOfOrNull {
                        findApIp() ?: run { Thread.sleep(500); null }
                    }
                    Handler(Looper.getMainLooper()).post {
                        if (ip != null) {
                            onReady(ip)
                        } else {
                            MushStore.update { s -> s.copy(statusLine = "hotspot up but no AP interface IP after 15s") }
                            onFailed(-2)
                        }
                    }
                }.start()
            }

            override fun onFailed(reason: Int) {
                MushStore.update { s -> s.copy(statusLine = "hotspot onFailed($reason)") }
                onFailed(reason)
            }

            override fun onStopped() {
                reservation = null
                MushStore.update { s -> s.copy(statusLine = "hotspot stopped") }
            }
        }
        wifi.startLocalOnlyHotspot(callback, Handler(Looper.getMainLooper()))
    }

    fun stopHotspot() {
        reservation?.close()
        reservation = null
    }

    /** IPv4 интерфейса точки доступа (или null, пока vendor-стек его не назначил). */
    fun findApIp(): String? =
        findIpv4 { name -> name.startsWith("wlan") || name.startsWith("ap") || name.startsWith("swlan") }

    /** LAN-адрес устройства (режим fallback: оба телефона в одной Wi-Fi сети). */
    fun lanIp(): String? = findIpv4 { name -> name.startsWith("wlan") || name.startsWith("eth") || name.startsWith("ccmni") }

    private fun findIpv4(ifaceMatch: (String) -> Boolean): String? =
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && ifaceMatch(it.name) }
            .flatMap { it.inetAddresses.toList() }
            .firstOrNull { it is Inet4Address && !it.isLoopbackAddress }
            ?.hostAddress
}
