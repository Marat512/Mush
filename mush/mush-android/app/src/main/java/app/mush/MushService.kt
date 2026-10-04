package app.mush

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/** Foreground service, держащий ноду (A'-ANDROID-SPEC §6.3).
 *
 * Без него Android убивает процесс через 5–10 минут в фоне. Service — владелец
 * native handle и MushCallback (§5.3: ссылка только на Service-owned объект);
 * UI общается через [NodeState].
 *
 * foregroundServiceType=connectedDevice заявлен в манифесте; трёхаргументный
 * startForeground доступен с API 29 (наш minSdk-target диапазон включает 29). */
class MushService : Service() {
    companion object {
        const val CHANNEL_ID = "mush"
        const val NOTIF_ID = 1
    }

    private var handle: Long = 0

    /** Service-owned callback (§5.3). Rust держит GlobalRef на этот объект
     * до shutdown; Activity сюда не попадает. */
    private val callback = object : MushCallback {
        override fun onMessage(groupId: String, senderPubkey: String, text: String, ts: Long) {
            NodeState.event("onMessage: $text")
            updateNotification("Message: $text")
        }

        override fun onSensorAlert(sensor: String, targetPubkey: String, reason: String) {
            NodeState.event("onSensorAlert[$sensor]: $reason")
            updateNotification("Alert [$sensor]")
        }

        override fun onHandshakeComplete(peerPubkey: String) {
            NodeState.event("onHandshakeComplete: ${peerPubkey.take(16)}…")
            updateNotification("Peer connected")
        }

        override fun onError(code: Int, message: String) {
            NodeState.event("onError($code): $message")
            updateNotification("Error ($code)")
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        createChannel()
        startForegroundWithType()
        if (handle == 0L) {
            Thread { startNode() }.start()
        }
        return START_STICKY
    }

    private fun startNode() {
        try {
            val h = Native.init(filesDir.absolutePath, callback)
            val pubkey = Native.pubkey(h)
            val port = Native.startListener(h, 0)
            val invite = Native.invite(h)
            handle = h
            NodeState.started(pubkey, port, invite)
            updateNotification("Node on port $port")
        } catch (t: Throwable) {
            NodeState.stopped("Error: ${t.message}")
        }
    }

    private fun createChannel() {
        val channel = NotificationChannel(CHANNEL_ID, "Mush", NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun notification(text: String): Notification =
        Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("Mush")
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_mush)
            .build()

    private fun startForegroundWithType() {
        val notif = notification("Starting…")
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(
                NOTIF_ID,
                notif,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
            )
        } else {
            startForeground(NOTIF_ID, notif)
        }
    }

    private fun updateNotification(text: String) {
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification(text))
    }

    override fun onDestroy() {
        if (handle != 0L) {
            runCatching { Native.shutdown(handle) }
            handle = 0
        }
        NodeState.stopped("Node stopped")
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
