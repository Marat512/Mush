package app.mush

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import app.mush.ui.ConnectionState
import app.mush.ui.MushStore
import app.mush.ui.NodeCommand
import app.mush.ui.NodePhase

/** Foreground service, держащий ноду (A'-ANDROID-SPEC §6.3).
 *
 * Без него Android убивает процесс через 5–10 минут в фоне. Service — владелец
 * native handle и Service-owned MushCallback (§5.3: ссылка только на
 * Service-owned объект); состояние кладёт в [MushStore], команды из UI
 * забирает через commandSink.
 *
 * foregroundServiceType=connectedDevice заявлен в манифесте; трёхаргументный
 * startForeground доступен с API 29 (наш minSdk-target диапазон включает 29). */
class MushService : Service() {
    companion object {
        const val CHANNEL_ID = "mush"
        const val NOTIF_ID = 1
        private const val PREFS = "mush_prefs"
        private const val KEY_NAME = "name"
    }

    private var handle: Long = 0
    private var myPubkey: String = ""
    private var lamportSeq = 0L

    /** Service-owned callback (§5.3). Rust держит GlobalRef на этот объект
     * до shutdown; Activity сюда не попадает. */
    private val callback = object : MushCallback {
        override fun onMessage(groupId: String, senderPubkey: String, text: String, ts: Long) {
            MushStore.update { s ->
                val name = s.contacts.firstOrNull { it.pubkey == senderPubkey }?.name
                    ?: senderPubkey.take(8)
                s.copy(
                    messages = (s.messages + app.mush.ui.ChatMessage(
                        id = "$ts:${s.messages.size}",
                        senderKey = senderPubkey,
                        senderName = if (senderPubkey == s.myPubkey) s.myName else name,
                        text = text,
                        ts = ts,
                        mine = senderPubkey == s.myPubkey,
                    )).takeLast(200),
                )
            }
            updateNotification("Message: $text")
        }

        override fun onSensorAlert(sensor: String, targetPubkey: String, reason: String) {
            MushStore.update { s ->
                s.copy(
                    alerts = s.alerts + app.mush.ui.AlertItem(
                        id = "${System.currentTimeMillis()}",
                        sensor = sensor,
                        targetPubkey = targetPubkey,
                        reason = reason,
                    ),
                )
            }
            updateNotification("Alert [$sensor]")
        }

        override fun onHandshakeComplete(peerPubkey: String) {
            MushStore.update { s ->
                val known = s.contacts.any { it.pubkey == peerPubkey }
                s.copy(
                    contacts = if (known) {
                        s.contacts.map { if (it.pubkey == peerPubkey) it.copy(connected = true) else it }
                    } else {
                        s.contacts + app.mush.ui.Contact(
                            pubkey = peerPubkey,
                            name = peerPubkey.take(8),
                            connected = true,
                        )
                    },
                )
            }
            updateNotification("Peer connected")
        }

        override fun onError(code: Int, message: String) {
            MushStore.update { it.copy(statusLine = "Error ($code): $message") }
            updateNotification("Error ($code)")
        }
    }

    override fun onCreate() {
        super.onCreate()
        MushStore.commandSink = { cmd -> handleCommand(cmd) }
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
        MushStore.update {
            it.copy(phase = NodePhase.Starting, statusLine = "Creating identity…")
        }
        try {
            val h = Native.init(filesDir.absolutePath, callback)
            val pubkey = Native.pubkey(h)
            val port = Native.startListener(h, 0)
            handle = h
            myPubkey = pubkey
            val name = getSharedPreferences(PREFS, MODE_PRIVATE).getString(KEY_NAME, "") ?: ""
            MushStore.update {
                it.copy(
                    phase = NodePhase.Running,
                    statusLine = "Node started on port $port",
                    myPubkey = pubkey,
                    myName = name,
                    invite = Native.invite(h),
                )
            }
            bringUpNetwork(port)
        } catch (t: Throwable) {
            MushStore.update {
                it.copy(phase = NodePhase.Error, statusLine = "Error: ${t.message}")
            }
        }
    }

    /** Hotspot — основной режим (§6.1); любая неудача → LAN fallback (§6.2).
     *  После подъёма интерфейса invite перевыпускаем с реальным IP. */
    private fun bringUpNetwork(port: Int) {
        NetworkManager.startHotspot(
            this,
            onReady = { ip ->
                Native.setAdvertisedAddr(handle, "$ip:$port")
                val invite = Native.invite(handle)
                MushStore.update {
                    it.copy(
                        connection = ConnectionState.Hotspot(ip, port),
                        invite = invite,
                        statusLine = "Hotspot $ip:$port",
                    )
                }
                updateNotification("Hotspot $ip:$port")
            },
            onFailed = {
                val lan = NetworkManager.lanIp()
                if (lan != null) {
                    Native.setAdvertisedAddr(handle, "$lan:$port")
                    MushStore.update { it.copy(connection = ConnectionState.Lan(lan, port)) }
                    updateNotification("LAN $lan:$port")
                } else {
                    MushStore.update { it.copy(connection = ConnectionState.None) }
                    updateNotification("Node on port $port")
                }
            },
        )
    }

    private fun handleCommand(cmd: NodeCommand) {
        if (handle == 0L) return
        when (cmd) {
            is NodeCommand.SendMessage -> {
                // Локальное эхо (callback Rust шлёт только входящие).
                MushStore.update { s ->
                    lamportSeq += 1
                    s.copy(
                        messages = (s.messages + app.mush.ui.ChatMessage(
                            id = "local:$lamportSeq",
                            senderKey = s.myPubkey,
                            senderName = s.myName,
                            text = cmd.text,
                            ts = lamportSeq,
                            mine = true,
                        )).takeLast(200),
                    )
                }
                Native.send(handle, "", cmd.text)
            }
            is NodeCommand.Join -> Native.join(handle, cmd.invite)
            is NodeCommand.DismissAlert -> MushStore.update { s ->
                s.copy(alerts = s.alerts.filterNot { it.id == cmd.id })
            }
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
        MushStore.commandSink = null
        NetworkManager.stopHotspot()
        if (handle != 0L) {
            runCatching { Native.shutdown(handle) }
            handle = 0
        }
        MushStore.update { it.copy(phase = NodePhase.Stopped, statusLine = "Node stopped") }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
