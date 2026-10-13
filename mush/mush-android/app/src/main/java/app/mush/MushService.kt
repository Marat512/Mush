package app.mush

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
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
    private var listeningPort = 0

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
            // Лог для hardware-прогона: ассерт доставки — grep по logcat
            // (тест-план §3.3, фильтр §6.3).
            Log.i("mush", "message from=${senderPubkey.take(12)} text=$text")
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
            Log.i("mush", "handshake complete peer=${peerPubkey.take(12)}")
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
            // В logcat тоже: иначе ошибки join/handshake невидимы в
            // adb-прогоне (notification без монитора не читается).
            Log.w("mush", "node error code=$code: $message")
            MushStore.update { it.copy(statusLine = "Error ($code): $message") }
            updateNotification("Error ($code)")
        }
    }

    override fun onCreate() {
        super.onCreate()
        MushStore.commandSink = { cmd -> handleCommand(cmd) }
        // Guard — debuggable APK (эквивалент BuildConfig.DEBUG, но без
        // buildFeatures.buildConfig = true: флаг уже есть у ApplicationInfo).
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            registerDebugReceiver()
        }
    }

    /** Debug-only (debuggable APK) хук для hardware-прогона: команды узла
     *  из adb, без UI и камеры (тест-план §3.2: 0.2 join/send, 0.5 group):
     *      am broadcast -a app.mush.debug.CMD --es cmd join  --es invite "<invite>"
     *      am broadcast -a app.mush.debug.CMD --es cmd send  --es text "hello"
     *      am broadcast -a app.mush.debug.CMD --es cmd flood --es n 50
     *      am broadcast -a app.mush.debug.CMD --es cmd decoy
     *  Receiver EXPORTED намеренно (shell uid 2000 шлёт broadcast);
     *  в не-debuggable сборке не регистрируется — ветки нет в коде. */
    private fun registerDebugReceiver() {
        registerReceiver(
            object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) {
                    when (intent.getStringExtra("cmd")) {
                        "join" -> intent.getStringExtra("invite")?.let {
                            Log.i("mush", "debug: join invited len=${it.length}")
                            MushStore.send(NodeCommand.Join(it))
                        }
                        "send" -> intent.getStringExtra("text")?.let {
                            MushStore.send(NodeCommand.SendMessage(it))
                        }
                        "flood" -> intent.getStringExtra("n")?.toIntOrNull()?.let {
                            MushStore.send(NodeCommand.DebugFlood(it))
                        }
                        "decoy" -> MushStore.send(NodeCommand.DebugDecoy)
                        "start" -> MushStore.send(NodeCommand.StartNetworking)
                        // Свежий invite (новый nonce + неиспользованный OPK).
                        // Нужен для мультиджойна в группу: bundle в invite
                        // одноразовый (OPK сжигается при первом handshake —
                        // tombstone от повторного использования, crypto/ratchet.rs),
                        // без перевыпуска второй джойнер получает reject.
                        // Тест-план §3.5; продуктовый auto-reissue — TODO Phase 1.
                        "reinvite" -> {
                            val h = handle
                            if (h != 0L) {
                                val inv = Native.invite(h)
                                Log.i("mush", "invite=$inv")
                            }
                        }
                        // Принудительный LAN-режим (debug): EMUI 10 гасит STA
                        // при подъёме softap на том же радио — клиенту чужого
                        // AP hotspot вредит. Закрываем reservation и
                        // адвертисим LAN-IP (retry: STA-реконнект занимает
                        // секунды). Тест-план §3.1 (LAN fallback), 0.5 (group).
                        "lan" -> forceLanMode()
                    }
                }
            },
            IntentFilter("app.mush.debug.CMD"),
            Context.RECEIVER_EXPORTED,
        )
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
     *  и setCryptoMode — до startListener, нода ещё не запущена.
     *
     *  Отказ Vault (DekError: dek.bin порчен/Keystore недоступен) — НЕ краш:
     *  нода живёт в памяти без persistence (как A'), statusLine честно
     *  сообщает. Временное поведение до UX-flow «Импорт бэкапа/Новая
     *  идентичность» (MUSH-B-SPEC §1.2.1). */
    private fun initNode() {
        MushStore.update {
            it.copy(phase = NodePhase.Starting, statusLine = "Creating identity…")
        }
        var vaultError: String? = null
        vault = try {
            VaultManager.open(this)
        } catch (e: DekError) {
            // Граница Result — VaultManager: сюда доходит уже DekError.
            android.util.Log.e("MushService", "Vault недоступен: ${e.message}", e)
            vaultError = e.message
            null
        }
        try {
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
                    statusLine = when {
                        vaultError != null ->
                            "Vault недоступен: $vaultError — identity эфемерна до восстановления"
                        restored != null -> "Identity restored from Vault"
                        else -> "Identity created"
                    },
                    myPubkey = pubkey,
                    myName = prefsName(),
                )
            }
            // Auto-resume (критерий 8 / soak): START_STICKY перезапускает
            // сервис БЕЗ Activity — UI-команды StartNetworking при этом нет,
            // и нода молчала бы вечно. Имя в prefs = onboarding пройден =
            // пользователь ждёт работающую ноду. startNetworking идемпотентен
            // (networkingStarted), повтор из UI/ChatsScreen безопасен.
            if (prefsName().isNotBlank()) startNetworking()
        } catch (t: Throwable) {
            MushStore.update {
                it.copy(phase = NodePhase.Error, statusLine = "Error: ${t.message}")
            }
        }
    }

    /** Контакты из Vault в MushStore (§1.3/§3.2): trust_level дочитывается
     *  реальный (verified | verification_failed), не дефолт. */
    private fun restoreContacts() {
        val stored = vault?.contacts() ?: return
        val b64 = stored.map {
            app.mush.ui.Contact(
                pubkey = android.util.Base64.encodeToString(it.pubkey, android.util.Base64.DEFAULT),
                name = it.name,
                connected = false, // connected — runtime-состояние, не persist
                trust = when (it.trust) {
                    TrustLevel.VERIFIED -> app.mush.ui.UiTrust.VERIFIED
                    TrustLevel.VERIFICATION_FAILED -> app.mush.ui.UiTrust.FAILED
                    TrustLevel.UNVERIFIED -> app.mush.ui.UiTrust.UNVERIFIED
                },
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
            listeningPort = port
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
                Log.i("mush", "networking ready: advertised $ip:$port")
                // Invite == содержимое QR (§5.0) — публичен по дизайну; лог
                // нужен скриптам прогона (клиент берёт строкой, без камеры).
                Log.i("mush", "invite=$invite")
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

    /** Debug (lan-команда): закрыть hotspot и адвертисить LAN-IP.
     *  STA-реконнект после stopHotspot занимает секунды — retry до 20 с. */
    private fun forceLanMode() {
        NetworkManager.stopHotspot()
        Thread {
            val ip = (1..40).firstNotNullOfOrNull {
                NetworkManager.lanIp()?.takeIf { ip -> !ip.startsWith("192.168.43.") }
                    ?: run { Thread.sleep(500); null }
            }
            if (ip != null && handle != 0L) {
                Native.setAdvertisedAddr(handle, "$ip:$listeningPort")
                MushStore.update {
                    it.copy(
                        connection = ConnectionState.Lan(ip, listeningPort),
                        statusLine = "LAN $ip:$listeningPort",
                    )
                }
                Log.i("mush", "lan mode: advertised $ip:$listeningPort")
            } else {
                Log.w("mush", "lan mode: no LAN ip after 20s")
            }
        }.start()
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
            is NodeCommand.SetTrust -> {
                val s = MushStore.state.value
                val contact = s.contacts.firstOrNull { it.pubkey == cmd.pubkey } ?: return
                val bytes = android.util.Base64.decode(cmd.pubkey, android.util.Base64.DEFAULT)
                val vaultLevel = when (cmd.trust) {
                    app.mush.ui.UiTrust.VERIFIED -> TrustLevel.VERIFIED
                    app.mush.ui.UiTrust.FAILED -> TrustLevel.VERIFICATION_FAILED
                    app.mush.ui.UiTrust.UNVERIFIED -> TrustLevel.UNVERIFIED
                }
                // Vault может быть недоступен (DekError): галочка живёт в
                // MushStore до рестарта, persist пропускаем молча (§1.2.1).
                runCatching {
                    vault?.upsertContact(bytes, contact.name, vaultLevel)
                }
                MushStore.update { st ->
                    st.copy(
                        contacts = st.contacts.map {
                            if (it.pubkey == cmd.pubkey) it.copy(trust = cmd.trust) else it
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
            // SetTrust/StartNetworking разбираются в handleCommand выше.
            is NodeCommand.SetTrust -> Unit
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
