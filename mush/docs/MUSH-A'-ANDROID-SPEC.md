# MUSH-A'-ANDROID-SPEC — Android-прототип (ТЗ2)

**Версия:** 0.1.0
**Дата:** 2026-10-04
**Статус:** Draft — ТЗ2, продолжение MUSH-A'-SPEC v0.1.1 / MUSH-A'-CLI-SPEC
**Предусловие:** CI зелёный на ubuntu + macOS (выполнено, run #3)
**Контракт:** protocol не меняется относительно Rust CLI. Меняются transport (TCP → Hotspot/LAN) и UI (TUI → Compose). Ядро — то же: `mush-core` через JNI.

---

## 0. Жёсткие рамки

**Не входит (scope freeze):**
- Этап A: Double Ratchet, pre-keys, BPv7 — НЕТ.
- Этап E: FSM, кворум, quarantine, CUSUM — НЕТ. `evidence_tier` остаётся заглушкой `Hard`.
- Push-уведомления, бэкапы, темы оформления, файлы, голосовые — НЕТ.
- BLE, WiFi-Direct — НЕТ (Hotspot/LAN only, по A'-SPEC §5.1–5.2).

**Входит:** ровно 8 критериев приёмки MUSH-A'-SPEC §8 на 2–4 физических телефонах/эмуляторах.

**Toolchain:**
- Android Studio (bundled JDK 17+, SDK 34, build-tools 34).
- NDK 26+ (`cargo-ndk` 3.x) — таргеты `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android` (последний — для эмулятора).
- Rust stable + `cargo-ndk`, `jni` crate 0.21.
- Kotlin, Jetpack Compose BOM 2024.09+, minSdk 26, targetSdk 34.

**Репо-структура (добавляется к существующему):**

```
mush/
├── mush-core/            # БЕЗ ИЗМЕНЕНИЙ (jar/CLI тоже без изменений)
├── mush-cli/             # БЕЗ ИЗМЕНЕНИЙ
└── mush-android/         # НОВОЕ: Gradle-проект
    ├── app/              # Kotlin + Compose UI
    │   └── src/main/...
    │       └── jniLibs/  # .so собираются cargo-ndk, не коммитятся
    ├── mush-jni/         # Rust crate: extern "C" мост над mush-core
    └── build.gradle.kts
```

---

## Фазы

### Фаза 0 — Milestone 0: «Hello from Rust» в APK (1 день)

**Цель:** доказать, что Rust-код исполняется внутри APK раньше любого UI-сложности. Если JNI-мост не работает — всё остальное бессмысленно.

1. `mush-jni` crate: `cdylib`, зависимость `mush-core`, `jni = "0.21"`.
2. Одна функция:

```rust
#[no_mangle]
pub extern "C" fn Java_app_mush_core_MushCore_hello<'local>(
    mut env: JNIEnv<'local>, _class: JClass<'local>
) -> JString<'local> {
    let identity = mush_core::identity::Identity::generate();
    let key = mush_core::base64::encode(&identity.pubkey_bytes());
    env.new_string(format!("Hello from Rust! Ed25519 key: {key}"))
       .expect("JVM string")
}
```

3. Kotlin: `object MushCore { init { System.loadLibrary("mush_jni") } external fun hello(): String }`
4. Compose: один экран, `Text(MushCore.hello())`, по центру.
5. Сборка: `cargo ndk -t aarch64 -t x86_64 -o app/src/main/jniLibs build --release`, далее `./gradlew assembleDebug`.
6. **Приёмка MS0:** APK установлен на эмуляторе (x86_64) и физическом устройстве (aarch64), на экране строка `Hello from Rust! Ed25519 key: <base64>` — разная на каждом устройстве (ключ генерируется при вызове).

**Риски:** cargo-ndk не видит NDK (флаг `-p`), ABI-несовпадение эмулятора (лечится x86_64 таргетом), `UnsatisfiedLinkError` (порядок `loadLibrary` / отсутствие .so в apk → проверить `unzip -l`).

### Фаза 1 — JNI-мост ядра (1–1.5 дня)

**Цель:** Kotlin управляет нодой `mush-core` как в CLI: lifecycle, команды, события.

Обернуть минимальный API (state machine ноды живёт в Rust, Kotlin — тонкий клиент):

| Rust (`mush-jni`) | Kotlin вызов | Комментарий |
|---|---|---|
| `nodeCreate(name, port) -> jlong` | `MushCore.create(name, port): Long` | handle = Box::into_raw(Node) |
| `nodeStart(handle)` | `start()` | spawn accept loop в tokio runtime (один глобальный) |
| `nodeInvite(handle) -> String` | `invite(): String` | `mush://` строка |
| `nodeJoin(handle, invite)` | `join(invite)` | блокирующий, из Dispatchers.IO |
| `nodeSend(handle, group, text)` | `send(group, text)` | |
| `nodePollEvents(handle) -> String[]` | `pollEvents(): List<String>` | JSON-строки событий: `message`, `alert`, `contact`, `status` |
| `nodeDestroy(handle)` | `close()` | drop Box, остановить runtime-задачи |

Правила JNI:
- Все `jlong`-handle'и валидируются (null → `MushJniException`).
- Ошибки Rust → `throw_new("app/mush/core/MushJniException", msg)`, не краш нативно.
- `pollEvents` — pull-модель (Kotlin тикает корутиной каждые 200 мс), потому что callback из Rust в Kotlin через `JavaVM` слишком хрупок для A'. Pull достаточен.
- Node не `Send` между потоками JVM — мост сериализует вызовы через один глобальный мьютекс (A' однопользовательский, норм).

**Приёмка:** юнит-тест на JVM-host ( Robolectric НЕ нужен — тесты только Rust-стороны JNI-функций с фейковым JNIEnv невозможны; вместо этого: `cargo test` в `mush-jni` покрывает логику обёртки без JVM) + инструментальный тест: create → invite → join (две ноды в одном процессе, порты 18001/18002) → send → pollEvents содержит `message`.

### Фаза 2 — Transport: Hotspot + LAN-fallback (1–1.5 дня)

**Цель:** заменить `127.0.0.1` на реальные устройства в одной комнате.

1. **Hotspot-режим (primary):** `WifiManager.startLocalOnlyHotspot()`. AP-имя `Mush-XXXX` (`group_id[0..4]` hex). Permissions: `NEARBY_WIFI_DEVICES` (API 33+, `neverForLocation`) / `ACCESS_FINE_LOCATION` (< 33). `LocalOnlyHotspotCallback` → onStarted даёт `WifiConfiguration` с softApConfiguration; IP owner'а — через `ConnectivityManager.getLinkProperties(network).linkAddresses` (НЕ хардкод 192.168.43.1).
2. **LAN-fallback (спека §5.1, зафиксирован):** если permission отклонён или hotspot не поднялся — оба устройства в одной внешней Wi-Fi сети; IP owner'а через тот же NetworkInterface API, в QR как обычно. UI-бейдж «LAN mode».
3. Клиент: сканирует SSID не нужно — IP+port приходят из QR (как в CLI invite). Подключение — обычный `java.net.Socket` / tokio TCP в Rust-ядре (transport.rs уже TCP — менять почти нечего, только bind 0.0.0.0 и адрес из invite).
4. Двусторонний обмен: owner accept'ит, joiner коннектится — симметрично CLI.

**Приёмка:** два эмулятора/телефона: owner поднимает hotspot (или LAN), joiner сканирует QR, handshake проходит, сообщение доходит. Airplane-mode проверка отложена до физических устройств (эмуляторы airplane-mode не делают).

### Фаза 3 — UI: Compose-экраны (1–2 дня)

Экраны строго по MUSH-A'-SPEC §7.1:

| Экран | Содержимое | MS |
|---|---|---|
| **Splash** | логотип + «Создаём ваш защищённый ключ…» (~2с, фактически nodeCreate) | 3 |
| **Onboarding** | имя + идентикон (генерация из pubkey: 5×5 зеркальная матрица, 2 цвета из blake3(pubkey)) + кнопка «Показать QR» | 3 |
| **Chats** | список групп, FAB «+» (создать / присоединиться по QR) | 3 |
| **Chat** | сообщения, статус ✓/✓✓/⏳, ввод | 3 |
| **Contacts** | список, «Поделиться контактом» (показ QR invite) | 3 |
| **Alerts** | алерты S1/S7, кнопки «Игнорировать» / «Заблокировать» (блокировка = локальный список, удаляет контакт) | 3 |
| **Debug (скрытый)** | кнопка «Отправить на decoy» (5 тапов по версии) | 3 |

QR: ZXing (`com.google.zxing:core` + `androidx.camera`) — генерировать invite-строку, сканировать. Invite deep-link (`https://mush.app/i#...`) — НЕ в scope (нет сайта); только QR.

**Приёмка:** все экраны навигабельны, monkey 30 мин без краша (Firebase Test Lab / `adb shell monkey`).

### Фаза 4 — Критерии приёмки §8 (1 день + демо)

| # | Критерий (MUSH-A'-SPEC §8) | Как проверяем |
|---|---|---|
| 1 | Два телефона в комнате обмениваются сообщениями | hotspot/LAN, /send → ✓✓ |
| 2 | QR-контакт без интернета | airplane mode, QR scan, handshake |
| 3 | Invite-link с автоподхватом | — (deep-link вне scope; замена: QR-автоподхват) |
| 4 | Групповой чат 4 человека | 4 устройства, синхронизация через relay |
| 5 | S1 детектирует аномалию | debug: 50 сообщений подряд → alert |
| 6 | S7 детектирует decoy | debug-кнопка → alert |
| 7 | Backpressure | 100+ сообщений, старые удалены |
| 8 | Нет крашей 30 мин | monkey |

**Демо:** 4 телефона, групповой чат, airplane mode.

---

## Порядок работ

```
Фаза 0 (MS0) → Фаза 1 → Фаза 2 → Фаза 3 → Фаза 4 → демо
   1 день     1–1.5 дня  1–1.5 дня  1–2 дня   1 день
```

Правило: фаза N+1 не начинается, пока фаза N не показала работающий артефакт на устройстве/эмуляторе. MS0 — раньше всех экранов, без исключений: если «Hello from Rust» не показан на экране телефона, Compose-код не пишется.

## Риск-реестр (обновлён)

| Риск | Вероятность | Митигация |
|---|---|---|
| cargo-ndk/NDK-версия | высокая | NDK 26 LTS, пин в `rust-toolchain` + CI-android job (ubuntu, `assembleDebug` без эмулятора) |
| JNI-краш по обработке ошибок | средняя | все panic границы → Result → Throwable; `catch_unwind` в каждом extern "C" |
| Hotspot permissions на китайских прошивках | высокая | LAN-fallback (спека §5.1), UI-бейдж |
| Эмулятор без WiFi hotspot | высокая | Фаза 2 проверяется на LAN-mode между двумя эмуляторами; hotspot — на физике |
| `mush-core` bind 127.0.0.1 | — | меняем на 0.0.0.0 в nodeCreate по флагу (CLI не ломается) |

## CI (добавить к существующему ci.yml)

Job `android-build` на ubuntu-latest: setup JDK 17, SDK 34, NDK 26, rust android targets; шаги: `cargo ndk build` (mush-jni) + `gradle assembleDebug`. Артефакт: APK. Эмуляторные тесты — вне CI (медленно), только ручной прогон фаз.
