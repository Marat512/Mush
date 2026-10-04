# MUSH-A'-ANDROID-SPEC — Android прототип (этап A')

**Версия:** 0.1.3  
**Дата:** 2026-09-30  
**Статус:** Draft — спецификация Android-прототипа  
**База:** MUSH-A'-SPEC.md v0.1.1  
**Предшественник:** MUSH-A'-CLI-SPEC.md v0.1.0 (Rust CLI, проверка логики ядра)  
**Цель:** Два телефона в одной комнате обмениваются сообщениями через hotspot, без интернета и сервера. Проверка UX-гипотезы: «люди готовы сканировать QR вместо регистрации».

---

**Отношение к CLI:** тот же протокол, то же ядро (`mush-core`), меняются только transport (TCP → Hotspot) и UI (TUI → Compose). Криптография, framing, CRDT, sensors — переиспользуются через JNI без изменений.

---

## 1. Что входит в A' Android

| Компонент | Реализация | Чего НЕТ |
|-----------|-----------|----------|
| **JNI-мост** | `cdylib` над `mush-core`, `extern "C"` | Полная FFI-обвязка (только нужные функции) |
| **Hotspot transport** | `WifiManager.startLocalOnlyHotspot` | WiFi-Direct P2P, BLE |
| **LAN fallback** | Оба телефона в одной Wi-Fi, IP через QR | mDNS, autodiscovery |
| **QR** | ZXing (scan + generate) | NFC, invite-link с deep link |
| **UI** | Jetpack Compose, 6 экранов | Темы, анимации, настройки |
| **Foreground service** | Одна нотификация, держит сеть | Background workers, push |
| **Sensors** | S1 threshold, S7 decoy — alerts в UI | CUSUM, FSM, silent mode |

---

## 2. Архитектура

```
┌──────────────────────────────────────────────┐
│              Kotlin / Compose UI             │
│  Splash → Onboarding → Chats → Chat          │
│              Contacts → Alerts               │
├──────────────────────────────────────────────┤
│              Kotlin Service Layer            │
│  MushService (foreground)                    │
│  NetworkManager (hotspot/LAN)                │
│  QrManager (scan/generate)                   │
│  StateFlow<UiState>                          │
├──────────────────────────────────────────────┤
│              JNI Bridge                      │
│  Native.kt (external fun declarations)       │
│  MushCallback (Kotlin → Rust callbacks)      │
├──────────────────────────────────────────────┤
│              mush-jni (Rust cdylib)          │
│  extern "C" wrappers                         │
│  Tokio runtime (on dedicated thread)         │
│  GlobalRef to Kotlin callback object         │
├──────────────────────────────────────────────┤
│              mush-core (Rust library)        │
│  identity, sns, framing, crdt, sensors       │
│  transport: TCP (LAN) — переиспользуется     │
└──────────────────────────────────────────────┘
```

**Ключевое:** `mush-core` не модифицируется. Hotspot поднимается в Kotlin, TCP-сокет между телефонами — тот же, что в CLI. `mush-core` думает, что работает на localhost.

---

## 3. Структура репозитория

```
mush/
├── Cargo.toml                    # workspace
├── mush-core/                    # Rust library (не меняется)
├── mush-cli/                     # Rust CLI (не меняется)
├── mush-jni/                     # НОВОЕ: cdylib для Android
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # extern "C" экспорты
│       ├── runtime.rs            # Tokio runtime singleton
│       ├── callback.rs           # GlobalRef + attach_current_thread
│       └── errors.rs             # Result → Throwable
└── mush-android/                 # НОВОЕ: Android Studio project
    ├── app/
    │   ├── build.gradle.kts
    │   ├── src/main/
    │   │   ├── AndroidManifest.xml
    │   │   ├── java/app/mush/
    │   │   │   ├── MainActivity.kt
    │   │   │   ├── Native.kt             # external fun + init
    │   │   │   ├── MushService.kt        # foreground service
    │   │   │   ├── NetworkManager.kt     # hotspot/LAN
    │   │   │   ├── QrManager.kt          # ZXing wrapper
    │   │   │   └── ui/                   # Compose screens
    │   │   └── res/
    │   └── jniLibs/                      # .so от cargo-ndk
    └── build.gradle.kts
```

---

## 4. Setup

### 4.1. Rust toolchain

```bash
# Установка NDK-таргетов
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android

# cargo-ndk
cargo install cargo-ndk

# Сборка .so
cd mush-jni
cargo ndk -t arm64-v8a -t armeabi-v7a -o ../mush-android/app/src/main/jniLibs build --release
```

NDK version: r26+ (для API 26+; у r25 ломается `libc++_shared.so` linking).

### 4.2. mush-jni/Cargo.toml

```toml
[package]
name = "mush-jni"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
mush-core = { path = "../mush-core" }
jni = "0.21"
tokio = { version = "1", features = ["rt-multi-thread"] }
once_cell = "1"
tracing = "0.1"
tracing-android = "0.2"

[profile.release]
lto = true
strip = "symbols"
```

### 4.3. app/build.gradle.kts (фрагмент)

```kotlin
android {
    compileSdk = 34
    defaultConfig {
        minSdk = 26
        targetSdk = 34
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }
    }
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")
}

dependencies {
    implementation("androidx.activity:activity-compose:1.8.0")
    implementation("androidx.navigation:navigation-compose:2.7.5")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.7.0")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    implementation("com.google.zxing:core:3.5.2")
}
```

---

## 5. JNI-контракт

### 5.0. Формат invite — единый источник истины

**QR несёт полную invite-строку** — результат `Invite::render` из `mush-core::transport`:

```
mush://<b64 pubkey>&nonce=<b64>&addr=host:port&group=<b64 group_id>-<b64 group_key>
```

Одна строка содержит всё, что нужно принимающей стороне: pubkey владельца, nonce рукопожатия, адрес для TCP-подключения и credentials группы. Принимающая сторона передаёт её в `join` (а JNI-слой — в `Node::join_peer`) **целиком, без сборки/парсинга на JNI-слое**. Один формат для hotspot-режима и для LAN fallback — разница только в том, какой IP попал в `addr` (hotspot-интерфейс vs LAN-интерфейс).

**Fingerprint / identity display** — отдельный формат, НЕ для join:

```
mush://<pubkey>#<nonce>
```

Используется только для отображения («вот мой ключ для сверки») и позже в Safety Number (этап B). В QR не кодируется.

### 5.1. Rust-side: extern "C" функции

```rust
// mush-jni/src/lib.rs
use jni::objects::{JClass, JString, JObject};
use jni::sys::{jstring, jlong, jboolean, jbyteArray, jint};
use jni::JNIEnv;

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_init(
    mut env: JNIEnv, _class: JClass,
    data_dir: JString,
    callback: JObject,       // Kotlin callback object
) -> jlong;                  // handle (pointer as i64)

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_pubkey(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
) -> jstring;                // base64 pubkey

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_invite(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
) -> jstring;                // полная invite-строка (Invite::render), см. §5.0

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_join(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
    invite: JString,
) -> jboolean;               // true if handshake initiated

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_start_listener(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
    port: jint,              // 0 = auto
) -> jint;                   // actual port

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_connect(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
    host: JString,
    port: jint,
) -> jboolean;

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_send(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
    group_id: JString,
    text: JString,
) -> jboolean;

#[no_mangle]
pub extern "system" fn Java_app_mush_Native_shutdown(
    mut env: JNIEnv, _class: JClass,
    handle: jlong,
);
```

### 5.2. Kotlin-side

```kotlin
// Native.kt
object Native {
    init { System.loadLibrary("mush_jni") }
    
    external fun init(dataDir: String, callback: MushCallback): Long
    external fun pubkey(handle: Long): String
    external fun invite(handle: Long): String
    external fun join(handle: Long, invite: String): Boolean
    external fun startListener(handle: Long, port: Int): Int
    external fun connect(handle: Long, host: String, port: Int): Boolean
    external fun send(handle: Long, groupId: String, text: String): Boolean
    external fun shutdown(handle: Long)
}

// MushCallback.kt — интерфейс, реализуемый Kotlin'ом; Rust вызывает его
interface MushCallback {
    fun onMessage(groupId: String, senderPubkey: String, text: String, ts: Long)
    fun onSensorAlert(sensor: String, targetPubkey: String, reason: String)
    fun onHandshakeComplete(peerPubkey: String)
    fun onError(code: Int, message: String)
}
```

### 5.3. Rust → Kotlin callback

```rust
// callback.rs
pub struct KotlinCallback {
    vm: JavaVM,
    obj: GlobalRef,
}

impl KotlinCallback {
    pub fn invoke_message(&self, group_id: &str, sender: &str, text: &str, ts: i64) {
        let mut env = self.vm.attach_current_thread().unwrap();
        let _ = env.call_method(
            &self.obj, "onMessage",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)V",
            &[
                JValue::Object(&env.new_string(group_id).unwrap().into()),
                JValue::Object(&env.new_string(sender).unwrap().into()),
                JValue::Object(&env.new_string(text).unwrap().into()),
                JValue::Long(ts),
            ],
        );
    }
    // аналогично для остальных
}
```

**Правило:** `attach_current_thread` на каждом callback. Не пытаться держать `JNIEnv` между вызовами — Android-потоки приходят и уходят.

**Bounded channel (обязательно):** события из Rust в Kotlin идут через канал с фиксированной ёмкостью. Если Kotlin не успевает обрабатывать — дропаются самые старые события, а не копятся бесконечно. Иначе event storm (S1 сработает 100 раз за секунду) положит heap.

**GlobalRef lifecycle (обязательно):** `delete_global_ref` в `shutdown`/`nodeDestroy`. Не держать ссылки на Activity — только на callback-объект, принадлежащий Service (жизненный цикл ≥ жизненного цикла ноды).

### 5.4. Tokio runtime

```rust
// runtime.rs
use once_cell::sync::OnceCell;
use tokio::runtime::Runtime;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();

pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}
```

Не использовать `block_on` на main thread. JNI-функции возвращают быстро; реальная работа — в runtime через `runtime().spawn(...)`. Результаты приходят в Kotlin через callback.

### 5.5. Ошибки

```rust
// errors.rs
pub fn throw(env: &mut JNIEnv, msg: &str) {
    let _ = env.throw_new("java/lang/RuntimeException", msg);
}
```

Все `extern "C"` функции: `match inner(...) { Ok(x) => x, Err(e) => { throw(&mut env, &e.to_string()); default_value } }`.

---

## 6. Transport: Hotspot + LAN fallback

### 6.1. Hotspot (основной)

```kotlin
// NetworkManager.kt
@RequiresApi(Build.VERSION_CODES.O)
fun startHotspot(context: Context, onReady: (String, Int) -> Unit) {
    val wifi = context.getSystemService(WifiManager::class.java)
    val callback = object : WifiManager.LocalOnlyHotspotCallback() {
        override fun onStarted(reservation: WifiManager.LocalOnlyHotspotReservation) {
            val iface = NetworkInterface.getNetworkInterfaces().toList()
                .firstOrNull { it.name.startsWith("wlan") && it.isUp }
            val ip = iface?.inetAddresses?.toList()
                ?.firstOrNull { it is Inet4Address }?.hostAddress
            onReady(ip ?: "192.168.43.1", PORT)
        }
        override fun onFailed(reason: Int) { /* fallback to LAN */ }
        override fun onStopped() { /* ... */ }
    }
    wifi.startLocalOnlyHotspot(callback, Handler(Looper.getMainLooper()))
}
```

IP не хардкодить. Получить через NetworkInterface API — Samsung/Xiaomi/Huawei дают разные.

**Vendor-факт (протокол испытаний, 2026-10-04):** на устройствах со STA+AP concurrency (Qualcomm, HyperOS) station-wlan0 с WAN-подсетью выигрывает гонку у настоящего AP-интерфейса. Поиск IP AP — двухфазный: сначала ТОЛЬКО ap*/swlan* (10 сек), потом fallback на wlan*. Реализация: `NetworkManager.findApIp(strict)`.

### 6.2. Wi-Fi credentials flow (спецификация, реализация позже)

**Проблема:** клиент должен подключиться к AP владельца ДО скана invite-QR, но пароль softap генерируется системой.

**Flow (целевой):**

1. QR несёт `{ssid, passphrase, ip, port, group_id, group_key}` (встраивается в invite-строку новым полем `wifi=` — спек-правка формата §5.0 на этапе реализации).
2. Клиент читает QR → `WifiNetworkSpecifier.Builder().setSsid(ssid).setWpa2Passphrase(passphrase)` → системный диалог подключения → подключился → тот же QR даёт invite → `join`.
3. Никакого ручного ввода пароля.

**Открытый вопрос (проверить на Xiaomi за 30 минут до реализации):** `LocalOnlyHotspotReservation.getSoftApConfiguration().passphrase` (API 30+) и `getWifiConfiguration().preSharedKey` (deprecated, API < 33) на HyperOS возвращают реальный passphrase или null/redacted? Текущие наблюдения: HyperOS НЕ отдаёт passphrase приложению (vendor redaction, в dumpsys `<non-empty>`). Если подтвердится — WifiNetworkSpecifier не сработает, нужен fallback (системный Wi-Fi-QR в формате `WIFI:S:...;T:WPA;P:...;;`, который сканирует сама система). **Реализация не раньше 2 недель** (требует телефоны); сейчас пароль показывается текстом на QR-экране (работает на EMUI 10, не работает на HyperOS).

### 6.3. LAN fallback

Если hotspot не поднялся (permissions, кастомная прошивка, ошибка API):

- Оба телефона в одной Wi-Fi сети.
- Owner показывает QR с той же полной invite-строкой (§5.0) — только `addr` содержит LAN-IP вместо hotspot-IP.
- Client сканирует → `join` → подключается по TCP.
- Тот же протокол, тот же framing. Только IP не из hotspot-интерфейса, а из LAN-интерфейса.

### 6.4. Vendor-протокол испытаний (зафиксировано 2026-10-04)

| Устройство | AP | Клиент | Заметки |
|------------|-----|--------|---------|
| Huawei Y5 2019 (EMUI 10, API 29) | ❌ **softap полностью изолирует клиентов от хоста** (ping/TCP 100% loss, проверено с 2-го устройства и с ПК) | ✅ | `ACCESS_FINE_LOCATION` + включённый GPS (иначе SecurityException при granted) |
| Xiaomi 2312FPCA6G (HyperOS, Android 16, API 36) | ⚠️ passphrase redacted (не отдаётся приложению); rate-limit на частые `startLocalOnlyHotspot` (onFailed=3); лечится fallback на живой ap0 | ✅ | STA+AP concurrency: отдельный `ap0` (10.x); advertised-addr фикс — двухфазный поиск IP |
| Pixel / Samsung A | ✅ ожидаемо | ✅ | третий телефон для демо |

Вывод по топологиям: роль AP недетерминирована — приложение должно уметь обе роли (текущее поведение: обе умеют, клиент дополнительно сканирует QR). Для демо с 3+ устройствами — Pixel/Samsung как AP.

### 6.5. Foreground service

```kotlin
// MushService.kt
class MushService : Service() {
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val channel = NotificationChannel("mush", "Mush", IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        
        val notif = Notification.Builder(this, "mush")
            .setContentTitle("Mush")
            .setContentText("Connected to network")
            .setSmallIcon(R.drawable.ic_mush)
            .build()
        
        startForeground(1, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        return START_STICKY
    }
}
```

Без foreground service Android убьёт процесс через 5–10 минут. Это не опционально.

---

## 7. Permissions

| Permission | Зачем | Когда |
|------------|-------|-------|
| **INTERNET** | TCP-сокеты | Всегда |
| **ACCESS_NETWORK_STATE** | Проверка доступности Wi-Fi | Всегда |
| **NEARBY_WIFI_DEVICES** (API 33+) | Hotspot, Wi-Fi scan | Runtime |
| **ACCESS_FINE_LOCATION** (API < 33) | Wi-Fi scan (legacy) | Runtime |
| **CAMERA** | QR-скан | Runtime |
| **FOREGROUND_SERVICE** | Service | Всегда |
| **FOREGROUND_SERVICE_CONNECTED_DEVICE** (API 34+) | Service type | Всегда |
| **POST_NOTIFICATIONS** (API 33+) | Foreground notification | Runtime |

Runtime permissions: hotspot и camera — не даётся автоматически. Обоснование в UI перед запросом: «Mush использует Wi-Fi для связи с другими устройствами без интернета».

Отказ от hotspot permission → LAN fallback. Не блокировать UI.

---

## 8. UI — Compose

### 8.1. Экраны

| Экран | Содержимое |
|-------|-----------|
| **Splash** | Логотип, spinner, «Создаём ключ...» (2 сек) |
| **Onboarding** | Поле имени, идентикон (сгенерирован), кнопка «Далее» |
| **Chats** | Список групп, FAB «+» → создать/присоединиться |
| **Chat** | LazyColumn сообщений, поле ввода, статус ✓/✓✓/⏳ |
| **Contacts** | Список контактов, кнопка «Показать QR» |
| **Alerts** | Список alerts от S1/S7, действия «Игнорировать» / «Заблокировать» |
| **QR Scan (modal)** | ZXing scanner view |
| **QR Show (modal)** | ZXing QR generator с invite-строкой |

### 8.2. State management

```kotlin
data class UiState(
    val messages: Map<String, List<Message>> = emptyMap(),
    val contacts: List<Contact> = emptyList(),
    val alerts: List<Alert> = emptyList(),
    val connection: ConnectionState = Disconnected,
)

class MushViewModel : ViewModel() {
    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()
    
    private val callback = object : MushCallback {
        override fun onMessage(g: String, s: String, t: String, ts: Long) {
            viewModelScope.launch {
                _state.update { it.copy(messages = it.messages + ...) }
            }
        }
        // ...
    }
}
```

Callback приходит из Rust-треда. Все обновления StateFlow — через `viewModelScope.launch` (main dispatcher).

---

## 9. Milestones

### Milestone 0 — JNI PoC (0.5–1 день)

Цель: доказать, что Rust собирается под Android и вызывается из Kotlin.

Разделён на два гейта — это две разные проверки:

**MS0-internal (закрывается первым):**

- [ ] cargo-ndk собирает `libmush_jni.so` для arm64-v8a.
- [ ] APK собирается и запускается (эмулятор допустим).
- [ ] Одна кнопка → `Native.pubkey(handle)` → строка на экране.
- [ ] Логи Rust видны в Logcat через tracing-android.

Этот гейт доказывает главное: JNI работает в ART, `libmush_jni.so` загружается, `extern "C"` вызывается из Kotlin. Если MS0-internal не проходит — дальше не идти. Это blocker.

**MS0-hardware (обязателен перед Milestone 2):**

- [ ] APK устанавливается и запускается на реальном устройстве.

Эмулятор не покажет поведение hotspot API (SecurityException на кастомных прошивках, нестандартные IP-диапазоны Xiaomi/Huawei), runtime permissions и LAN-интерфейсы. Начинать Milestone 2 (Transport) без реального устройства нельзя — всё встанет на первом же тесте транспорта.

### Milestone 1 — JNI bridge полный (1 день)

- [ ] Все `extern "C"` функции из §5.1 реализованы.
- [ ] Callback Rust → Kotlin работает (тест: send to self, сообщение появляется в StateFlow).
- [ ] Ошибки → RuntimeException в Kotlin.
- [ ] `cargo test --release` для mush-jni проходит.

### Milestone 2 — Transport (1–1.5 дня)

- [ ] `WifiManager.startLocalOnlyHotspot` работает на тестовом устройстве.
- [ ] Owner IP определяется через NetworkInterface.
- [ ] TCP listener + connect между двумя телефонами.
- [ ] LAN fallback: если hotspot падает — переключение без перезапуска.
- [ ] Foreground service держит сеть >30 минут.
- [ ] Проверка: два телефона в одной комнате пингуют друг друга.

### Milestone 3 — Compose UI (1.5 дня)

- [ ] 6 экранов рендерятся, навигация работает.
- [ ] QR scan (ZXing) читает invite-строку.
- [ ] QR show генерирует invite-строку.
- [ ] Messages отображаются с статусами.
- [ ] Alerts от S1/S7 появляются в UI.

### Milestone 4 — Интеграция (1–1.5 дня)

- [x] End-to-end: 2 телефона, QR-контакт, /send, сообщение в чате. (принято 2026-10-04, Xiaomi=AP + Huawei=клиент)
- [x] Все 8 критериев из §10 проходят. (#4 группа-4 — осознанно отложена, см. ниже)
- [x] `cargo test` + `./gradlew test` зелёные.

**MS5 (группа 4 человека) — статус: отложена.** В Android-спеке, но по логике идёт ПОСЛЕ этапа A (Double Ratchet + group key rotation): группа на 4 нодах сейчас даст мало нового — CRDT merge уже проверен в CLI (3 ноды converge). Возвращается после этапа A, когда появится что ротировать в group key.

**Итого: 5–6 дней.** Не 3. JNI + hotspot + foreground service — три вещи, которые съедают больше, чем планируешь.

---

## 10. Критерии приёмки

Из MUSH-A'-SPEC.md §8 — 8 критериев, здесь они переформулированы для Android:

| # | Критерий | Как проверить |
|---|----------|--------------|
| 1 | Два телефона обмениваются сообщениями | Демо: 2 Android, hotspot, текст |
| 2 | QR-контакт работает без интернета | Airplane mode, QR scan, handshake |
| 3 | LAN fallback работает | Отключить hotspot permission → оба в одной Wi-Fi → сообщение доходит |
| 4 | Групповой чат 4 человека синхронизируется | 4 телефона, сообщения видны всем после sync |
| 5 | S1 детектирует аномалию | `/flood 50` в debug-меню → alert в UI |
| 6 | S7 детектирует decoy hit | Debug: «отправить на decoy» → alert |
| 7 | Backpressure работает | 100+ сообщений → oldest удаляются |
| 8 | Нет крашей при 30 мин | Monkey test, foreground service жив |

---

## 11. Что НЕ входит

| Компонент | Почему не в A' | Когда |
|-----------|---------------|-------|
| **Double Ratchet** | Session_key симметричный, достаточно | Этап A |
| **Pre-keys** | Нет offline delivery | Этап A |
| **BLE** | Android 12+ permissions + GATT MTU | Этап C |
| **WiFi-Direct P2P** | Нестабильно на Android | Этап C |
| **Onion + cover traffic** | Нет multi-hop в одной комнате | Этап C |
| **Fruiting Gateway** | Нет persistent групп | Этап D |
| **Immune plane** (FSM, кворум) | Только S1 + S7 alerts | Этап E |
| **Vironet** | Нет эволюции в 2-телефонном демо | Этап E |
| **Push-уведомления** | Только foreground notification | После A' |
| **Invite-link** (deep link) | Только QR в A' | Этап B |
| **Идентикон** | Простой цветной квадрат вместо | Этап B |
| **Safety Number** | Только fingerprint в debug | Этап B |

---

## 12. Риски

| Риск | Вероятность | Митигация |
|------|------------|-----------|
| **JNI не собирается** | Средняя | Milestone 0 — раньше всего. Если не проходит — блокер. |
| **Tokio блокирует main thread** | Средняя | `runtime().spawn()`, callbacks через StateFlow. Явно в §5.4. |
| **Hotspot падает на кастомной прошивке** | Высокая | LAN fallback в §6.2. Протестировать до демо. |
| **Foreground service убит Android** | Средняя | `START_STICKY` + `ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE`. |
| **Permissions отклонены** | Средняя | Грациозный fallback: hotspot → LAN. Обоснование в UI. |
| **NDK version mismatch** | Низкая | r26+, зафиксировать в README. |
| **Compose UI не успевает** | Низкая | Только 6 экранов, без тем/анимаций. |

---

## 13. Контракт с mush-core

**`mush-core` не знает про JNI-типы.**

API: `Identity`, `HandshakeInitiator`/`HandshakeResponder`, `Frame`/`framing`, `AdHocGroup`, `ContactRateSensor`, `DecoyEidSensor`, `InboxBuffer`, `Invite`, `Node` (+ опциональный push-sink `NodeEvent`, см. `set_event_sink`).

`mush-jni` — тонкий слой: только `extern "C"` обёртки, Tokio runtime, callback bridge.

**Правило расширения ядра:** generic-точки расширения контракта (например, push-события, новые accessor'ы) допустимы, если они не зависят от JNI/Android и не меняют протокол. Если для Android нужно что-то JNI-специфичное в `mush-core` — это признак, что JNI-слой протекает. Остановиться и пересмотреть.

---

## 14. Следующий шаг после A' Android

**Если A' проходит 8 критериев:**

- **Этап A:** Double Ratchet, pre-keys, persistent 1-on-1.
- **Этап B:** SQLCipher, Vault, идентикон, Safety Number.
- **Этап C:** BLE, WiFi-Direct, MAC rotation, Onion.
- **Этап D:** Fruiting Gateway, Group CRDT, SBP.
- **Этап E:** Immune plane, Vironet, CUSUM, stress-test.

**Если A' не проходит** (люди не сканируют QR, UX непонятен, hotspot не работает):

- Переделать onboarding и transport.
- Не добавлять сложности.
- Не начинать этап A.