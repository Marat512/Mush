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
    /** Listener/сеть подняты (StartNetworking от Onboarding/autoStart). */
    private var networkingStarted = false

    /** Этап B: внутренняя persistence узла (MUSH-B-SPEC §1). Это НЕ
     * user-facing Backup: флаг «export/import только явное действие» (§4)
     * про бэкап в файл — здесь throttled-автосохранение состояния ноды
     * (identity seed + pre-keys + сессии) в Vault той же логики касается. */
    private var vault: VaultManager? = null

    /** Throttle автосохранения: не чаще раза в 10 секунд (§7 дни 4–5). */
    private var lastVaultSaveMs = 0L
    private fun saveNodeStateThrottled(force: Boolean = false) {
        val h = handle
        val v = vault ?: return
        if (h == 0L) return
        val now = System.currentTimeMillis()
        if (!force && now - lastVaultSaveMs < 10_000) return
        lastVaultSaveMs = now
        // exportState — чистый снапшот (&self, идемпотентен); пишем мимо UI.
        runCatching { v.saveNodeState(Native.exportState(h)) }
    }

    /** Service-owned callback (§5.3). Rust держит GlobalRef на этот объект
     * до shutdown; Activity сюда не попадает. */
    private val callback = object : MushCallback {
        override fun onMessage(groupId: String, senderPubkey: String, text: String, ts: Long) {
            saveNodeStateThrottled()
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
            saveNodeStateThrottled()
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
            // Контакт persist в Vault (§1.3): переживает рестарт. Имя/trust
            // не трогаем — verified ставит только сверка safety number (§3.1).
            runCatching {
                vault?.addContactIfAbsent(
                    android.util.Base64.decode(peerPubkey, android.util.Base64.DEFAULT),
                    peerPubkey.take(8),
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
            // Флаг 4 (день 6): identity создаётся ЗДЕСЬ, при старте сервиса —
            // до Onboarding. Splash ждёт myPubkey в MushStore, Onboarding
            // только пишет имя и дёргает StartNetworking.
            Thread { initNode() }.start()
        }
        return START_STICKY
    }

    /** Identity + Vault restore. Без listener: сеть поднимается по
     *  StartNetworking (после Onboarding), контракты соблюдены: importState
     *  и setCryptoMode — до startListener, нода ещё не запущена. */
    private fun initNode() {
        MushStore.update {
            it.copy(phase = NodePhase.Starting, statusLine = "Creating identity…")
        }
        try {
            // Vault открываем до ноды — restore возможен только до старта
            // (importState заменяет ноду, §4 контракт «только на остановленной»).
            vault = VaultManager.open(this)
            val h = Native.init(filesDir.absolutePath, callback)
            val restored = vault?.loadNodeState()?.takeIf { Native.importState(h, it) }
            // Android всегда ratchet-режим (X3DH + Double Ratchet) — до
            // startListener, иначе пользователи получили бы legacy E2E.
            Native.setCryptoMode(h, 1)
            val pubkey = Native.pubkey(h)
            handle = h
            myPubkey = pubkey
            restoreContacts()
            MushStore.update {
                it.copy(
                    phase = NodePhase.Starting, // listener ещё не поднят
                    statusLine = if (restored != null) {
                        "Identity restored from Vault"
                    } else {
                        "Identity created"
                    },
                    myPubkey = pubkey,
                    myName = prefsName(),
                )
            }
        } catch (t: Throwable) {
            MushStore.update {
                it.copy(phase = NodePhase.Error, statusLine = "Error: ${t.message}")
            }
        }
    }

    /** Контакты из Vault в MushStore (§1.3): trust_level дочитывается
     *  реальный, не дефолт — иначе Verified терялся бы при рестарте. */
    private fun restoreContacts() {
        val stored = vault?.contacts() ?: return
        val b64 = stored.map {
            app.mush.ui.Contact(
                pubkey = android.util.Base64.encodeToString(it.pubkey, android.util.Base64.DEFAULT),
                name = it.name,
                connected = false, // connected — runtime-состояние, не persist
                verified = it.trust == TrustLevel.VERIFIED,
            )
        }
        MushStore.update { s -> s.copy(contacts = b64) }
    }

    private fun prefsName(): String =
        getSharedPreferences(PREFS, MODE_PRIVATE).getString(KEY_NAME, "") ?: ""

    /** Listener + hotspot/LAN + invite. Вызывается по StartNetworking. */
    private fun startNetworking() {
        val h = handle
        if (h == 0L || networkingStarted) return
        networkingStarted = true
        try {
            val port = Native.startListener(h, 0)
            MushStore.update {
                it.copy(
                    phase = NodePhase.Running,
                    statusLine = "Node started on port $port",
                    myName = prefsName(),
                    invite = Native.invite(h),
                )
            }
            // Начальный снапшот в Vault после подъёма listener.
            saveNodeStateThrottled(force = true)
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
                // AP может существовать, хотя резервация не взялась
                // (vendor rate-limit) — тогда адvertисим ap0.
                val ap = NetworkManager.findApIp(strict = true)
                val reachable = ap ?: NetworkManager.lanIp()
                if (reachable != null) {
                    Native.setAdvertisedAddr(handle, "$reachable:$port")
                    val invite = Native.invite(handle)
                    val mode = if (ap != null) ConnectionState.Hotspot(ap, port) else ConnectionState.Lan(reachable, port)
                    MushStore.update {
                        it.copy(connection = mode, invite = invite, statusLine = "${if (ap != null) "Hotspot" else "LAN"} $reachable:$port")
                    }
                    updateNotification("${if (ap != null) "Hotspot" else "LAN"} $reachable:$port")
                } else {
                    MushStore.update { it.copy(connection = ConnectionState.None) }
                    updateNotification("Node on port $port")
                }
            },
        )
    }

    private fun handleCommand(cmd: NodeCommand) {
        when (cmd) {
            // StartNetworking — до handle (identity ещё создаётся): стартуем
            // сервис (initNode отработает) и дожидаемся — polling короткий.
            NodeCommand.StartNetworking -> {
                startForegroundService(Intent(this, MushService::class.java))
                Thread {
                    val deadline = System.currentTimeMillis() + 15_000
                    while (handle == 0L && System.currentTimeMillis() < deadline) {
                        Thread.sleep(100)
                    }
                    startNetworking()
                }.start()
            }
            is NodeCommand.MarkVerified -> {
                val s = MushStore.state.value
                val contact = s.contacts.firstOrNull { it.pubkey == cmd.pubkey } ?: return
                val bytes = android.util.Base64.decode(cmd.pubkey, android.util.Base64.DEFAULT)
                runCatching {
                    vault?.upsertContact(bytes, contact.name, TrustLevel.VERIFIED)
                }
                MushStore.update { st ->
                    st.copy(
                        contacts = st.contacts.map {
                            if (it.pubkey == cmd.pubkey) it.copy(verified = true) else it
                        },
                    )
                }
            }
            else -> {
                if (handle == 0L) return
                handleNodeCommand(cmd)
            }
        }
    }

    private fun handleNodeCommand(cmd: NodeCommand) {
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
            // MarkVerified/StartNetworking разбираются в handleCommand выше.
            is NodeCommand.MarkVerified -> Unit
            NodeCommand.StartNetworking -> Unit
            is NodeCommand.DebugFlood -> Native.flood(handle, cmd.n)
            NodeCommand.DebugDecoy -> Native.decoy(handle)
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
            // Финальный снапшот до shutdown: после него handle мёртв.
            saveNodeStateThrottled(force = true)
            runCatching { Native.shutdown(handle) }
            handle = 0
        }
        vault?.close()
        vault = null
        MushStore.update { it.copy(phase = NodePhase.Stopped, statusLine = "Node stopped") }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
