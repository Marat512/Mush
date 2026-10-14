# Mush

**Mush** — мессенджер, который работает без интернета и без серверов: плодовое тело поверх DTN-грибницы [Mycerus](#архитектура). Пользователь видит простое приложение — под землёй store-carry-forward, градиентная маршрутизация и иммунная система сети.

Три столпа: **простой** (как Telegram, но без сервера) · **надёжный** (как почта, но быстрее) · **конфиденциальный** (как личная встреча, но на расстоянии).

## Статус

Реализовано и принято (все критерии пройдены, CI зелёный):

- **A'-CLI** ([MUSH-A'-CLI-SPEC](MUSH-A'-CLI-SPEC.md)): Rust-ноды на localhost, SNS-handshake, CRDT на 3 нодах, сенсоры S1/S7, smoke-скрипт.
- **A'-Android** ([MUSH-A'-ANDROID-SPEC](MUSH-A'-ANDROID-SPEC.md)): JNI-мост (mush-jni), foreground service, hotspot/LAN-fallback, Compose UI (6 экранов + QR ZXing), интеграция на двух физических устройствах (Huawei Y5 2019 + Xiaomi HyperOS). Vendor-протокол испытаний — §6.4 спеки.
- **Этап A — E2E-криптография** ([mush/docs/MUSH-A-SPEC.md](mush/docs/MUSH-A-SPEC.md)): X3DH + Double Ratchet в `mush-core::crypto::ratchet` (golden vectors, out-of-order, anti-replay, FS-тесты, session persistence), transport dual-mode (`CryptoMode::Legacy | Ratchet`), 4-нодная интеграция с relay-перешифровкой, восстановление после потерь в DTN.

CLI quickstart (default — ratchet-режим, X3DH + Double Ratchet; `--crypto legacy` — старый SNS-handshake, deprecated до этапа C):

```
cd mush
cargo run --release --bin mush-node -- --name Alice --port 7878
# второй терминал:
cargo run --release --bin mush-node -- --name Bob --port 7879 --join mush://<invite из stdout Alice>
```

Команды ноды: `/send /join /invite /contacts /sync /flood /decoy /alert /quit` (см. `/help`).

```bash
cargo test --release   # 81 тест: crypto, ratchet, handshake (два режима), CRDT, sensors, 2/3/4-нодные интеграционные
bash mush/scripts/smoke.sh  # smoke: два реальных процесса, 6 проверок
```

### Сборка APK для hardware-тестов

```bash
cd mush
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
  -o mush-android/app/src/main/jniLibs build -p mush-jni   # .so → jniLibs
cd mush-android && ./gradlew assembleDebug                 # app-debug.apk
```

> ⚠️ **Не экспортируйте `ANDROID_SDK_HOME` при запуске Gradle.** AGP ищет
> `debug.keystore` в `$ANDROID_SDK_HOME/.android` и, не найдя, **генерирует
> новый** — APK подпишется чужим ключом, и `adb install -r` на устройствах
> упадёт с `INSTALL_FAILED_UPDATE_INCOMPATIBLE` (разбор: MUSH-HARDWARE-TEST-PLAN
> §3.5 п.18д, 14.10). Для cargo-ndk SDK указывайте через `NDK_HOME`, а Gradle
> пусть берёт sdk.dir из `mush-android/local.properties`.

Что проверено: SNS-handshake (Ed25519 + X25519 + HKDF), CRDT-сходимость на 3 нодах, сенсоры S1/S7, backpressure, one-time invite, X3DH + Double Ratchet (этап A: pairwise E2E, FS, out-of-order, persistence), Android-прототип (hotspot, QR, foreground service). Чего нет (этапы B–E): Vault/SQLCipher, Safety Number, Onion, Fruiting Gateway + group E2E (sender keys), Immune plane, Vironet.

**Следующий:** этап B (Vault, SQLCipher, Identicon, Safety Number) + Android hardware-проверка на Pixel/Samsung. Group E2E через sender keys — этап D. Документированное ограничение A': relay групповых сообщений видит plaintext (ATTACK-MODEL §2, MUSH-A-SPEC §4.0).

## Документация

| Документ | Что это |
|----------|---------|
| [MUSH.md](MUSH.md) | Манифест: философия, UX-принципы, пользовательские сценарии |
| [MUSH-ARCH.md](MUSH-ARCH.md) | Техническая архитектура: компоненты, протоколы, роадмап A'→E |
| [MUSH-A'-SPEC.md](MUSH-A'-SPEC.md) | Спека первого прототипа A' (Android-цель) |
| [MUSH-A'-CLI-SPEC.md](MUSH-A'-CLI-SPEC.md) | Спека Rust CLI прототипа (текущая реализация) |
| [MUSH-PROTOCOL.md](MUSH-PROTOCOL.md) | Формальные протоколы: Immune Block 202, CUSUM, Quorum, FSM |
| [IMMUNE-SPEC-v0.1.md](IMMUNE-SPEC-v0.1.md) | Спецификация Immune Plane |
| [ATTACK-MODEL.md](ATTACK-MODEL.md) | Модель угроз Mycerus: 6 категорий атак, 6 слоёв иммунитета |
| [MUSH-BRIDGE-ARCH.md](MUSH-BRIDGE-ARCH.md) | Адаптеры сторонних сетей (B-статус, не в MVP) |

## Архитектура

```
Пользовательский слой   Mush Client (телефоны, десктоп) — "плодовые тела"
        │  WiFi-Direct / BLE / LAN
Грибница                Mycerus Mesh — DTN-роутеры: MGR, SNS, POT, Immune plane
        │  опционально
        ▼
                  Internet Bridge (fallback)
```

- **E2E-шифрование:** Noise/AES-256-GCM, ключ = Ed25519, forward secrecy на этапе A (Double Ratchet).
- **Метаданные минимизированы:** BPv7 source/destination — это pubkey-EID, содержимое не видно сети.
- **Спам структурно затруднён:** адрес = согласие (QR/NFC + SNS-handshake), группа = своя грибница (Fruiting Gateway); если прорвался — CUSUM-сенсоры и Trust FSM.

## Структура репо

```
mush/                  Rust workspace (A'-CLI прототип)
├── mush-core/         библиотека: identity, crypto, sns, framing, crdt, sensors, transport
├── mush-cli/          бинарь mush-node (clap + tokio)
├── scripts/smoke.sh   smoke-тест на два процесса
└── docs/              копии спек A' для офлайн-работы
*.md                   спецификации проекта (корень)
```

## Лицензия

Код и документация — [GNU AGPL v3.0 or later](LICENSE), соответствует некоммерческой природе проекта (MUSH.md §8: open-source, не стартап).
