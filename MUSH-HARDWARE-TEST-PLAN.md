# MUSH-HARDWARE-TEST-PLAN — окончательный план тестов на парке

Версия: 1.2.0
Дата: 2026-10-13
Статус: Final — исполнительный документ
Покрытие: A' Android (MS0–MS5) + этап A (Ratchet) + этап B (Vault, Identicon, Safety Number) на реальном железе

Изменения v1.1.0 → v1.2.0:

- §4.1: добавлен пункт про кабели в pre-flight.
- §4.2: метки метрик B.2/B.3/B.4 → M.1/M.2/M.3 (устранена коллизия с этапом B §5.3).
- §4.2: B.1 (persistence) → R.1 (Restart-persistence), устранена коллизия с §5.3 B.1.
- §4.2: MS6 и буфер разнесены по дням — день 13 = MS6, день 14 = буфер.
- §7: SQLCipher stop-condition помечен MS6 only.
- §10.3: уточнено, какие три файла обновляются и как.

## 1. Цель и scope

Цель: подтвердить, что реализованное в mush-core + mush-jni + mush-android работает на реальном парке устройств, а не только на эмуляторе и в CI.

Что уже закрыто без железа:

- MS0-internal (эмулятор), MS1–MS4 (Huawei Y5, Xiaomi HyperOS)
- Этап A: Double Ratchet, pre-keys, persistence, FS-проверки
- Этап B: Vault, SQLCipher, Identicon, Safety Number, три состояния TrustLevel

Что осталось за железом:

- Hotspot AP на реальных вендорах (POCO HyperOS rate-limit, Samsung Global)
- Keystore-обёртка DEK (гейт §6 — снять после hardware-verify) → MS6
- Групповой чат 4–6 устройств (MS5)
- Поведенческий тест verify → re-handshake → Verified на двух устройствах
- Restart-persistence (ручной скрипт R.1)
- minSdk boundary (Redmi 5, API 27)

Вне scope:

- iOS / iPhone 16 Pro Max — этап B-status (bridge), отдельный проект
- Android Go, планшеты, feature phones — этап C+

## 2. Парк устройств — роли

| Устройство | Serial | API | Release | ABI | Роль |
|---|---|---|---|---|---|
| Samsung A36 Global | (в пути) | 35 | 15 | arm64-v8a | Owner / Reference |
| POCO M6 Pro | eezpcaoznjrs5xxs | 36 | 16, HyperOS 3.0 | arm64-v8a | Stress Client (owner №1 в Phase 0) |
| Xiaomi Mi 9T | f740d46d | 30 | 11, MIUI | arm64-v8a | Stable Client (owner №2 fallback) |
| Huawei STK-LX1 | MVS4C20509005466 | 29 | 10, EMUI | arm64-v8a | LAN fallback / Client |
| Redmi 5 | 2bd836517d84 | 27 | 8.1 | arm64-v8a | minSdk boundary |
| Эмулятор | emulator-5554 | 30 | 11 | x86_64 | Rehearsal |
| iPhone 16 Pro Max | — | — | — | — | Out of scope |
| Samsung (1 шт. в пути) | — | 35 | 15 | arm64-v8a | Второй экземпляр (опционально) |

Покрытие API: 27 → 29 → 30 → 35 → 36. Пять точек.
Покрытие вендоров: MIUI, HyperOS, EMUI, OneUI (в пути), AOSP (эмулятор).
Покрытие чипсетов: Snapdragon (POCO, Mi 9T), Kirin (Huawei), Exynos (Samsung).

## 3. Phase 0 — до приезда Samsung (сейчас)

Samsung в пути. Работаем на 4 физических + эмулятор. Не блокируемся.

### 3.1. Порядок owner'ов (fallback)

POCO M6 Pro — owner №1. HyperOS имеет rate-limit на startLocalOnlyHotspot (onFailed 3). Одна попытка → fallback, не отлаживать.

| # | Owner | Ожидание |
|---|---|---|
| 1 | POCO M6 Pro | HyperOS rate-limit — риск onFailed |
| 2 | Xiaomi Mi 9T | MIUI 12, старше — rate-limit не было |
| 3 | Huawei STK-LX1 | EMUI, hotspot часто не работает |
| 4 | LAN fallback | Не требует hotspot, оба в одной Wi-Fi |

### 3.2. Задачи Phase 0

| # | Устройства | Задача | PASS | FAIL |
|---|---|---|---|---|
| 0.1 | POCO (owner) + Mi 9T (client) | Hotspot на POCO, Mi 9T коннектится, ping по IP | Пакеты ходят | onFailed 3 → owner №2 |
| 0.2 | POCO + Mi 9T | QR-контакт, handshake, /send hello | Сообщение у обоих | Расшифровка падает |
| 0.3 | Huawei + Mi 9T | LAN fallback: оба в одной Wi-Fi, IP из QR | Сообщение доходит | TCP connect fail |
| 0.4 | Redmi 5 | adb install APK, старт | Запустилось | INSTALL_FAILED_OLDER_SDK → out |
| 0.5 | 3 устройства | Групповой чат (POCO + Mi 9T + Huawei) | История идентична | CRDT merge расходится |
| 0.5b | Redmi 5 | Отдельно: minSdk validation как клиент на LAN | Запустилось / работает | INSTALL_FAILED или runtime crash |

### 3.3. Что смотреть в логе при Phase 0.1

После «Start networking» на POCO:

```bash
adb -s eezpcaoznjrs5xxs logcat -s mush:D AndroidRuntime:E
```

Три строки:

1. `listener started on 0.0.0.0:<port>` — не 127.0.0.1. Если 127.0.0.1 — баг, клиент не подключится.
2. `LocalOnlyHotspotCallback::onStarted` — с реальным IP 192.168.x.1.
3. `LocalOnlyHotspotCallback::onFailed` — reason=N, N=3 → rate-limit.

Стоп-условие: если 0.0.0.0 не появляется — искать в коде, где listener создаётся. Это блокер, не отлаживать hotspot дальше.

### 3.4. Redmi 5 — решение

```text
API 27 ≥ minSdk 26 → in scope.
```

Прогнать `adb -s 2bd836517d84 install -r app-debug.apk`. Если установилось — Redmi 5 в парке как клиент на LAN. Особенности:

- ACCESS_FINE_LOCATION + GPS toggle обязательны.
- startLocalOnlyHotspot с API 26, но реализация отличается от 29+. На Redmi 5 owner не проверять — только клиент.

### 3.5. Находки Phase 0 (2026-10-13, прогон на POCO + Huawei)

Зафиксировано на железе до закрытия 0.1–0.2:

1. **POCO M6 Pro (HyperOS 3.0, API 36): hotspot поднимается стабильно** — `onStarted` с IP 10.14.255.160 на ap0, без rate-limit (3 прогона подряд). Креды random per start (`AndroidShare_NNNN`), в logcat (debug APK) пишутся для скриптов.
2. **POCO: входящий TCP к приложению ЗАБЛОКИРОВАН вендорским firewall** — listener жив (проверено через `adb forward`), ping ходит, TCP-connect с пира таймаутится на обоих интерфейсах (wlan0 и ap0). Модель «клиент коннектится к owner по IP:port» на HyperOS не работает без вендорного whitelist (проверить Security app → сетевые разрешения для Mush — руками на устройстве).
3. **Huawei STK-LX1 (EMUI 10, API 29): hotspot РАБОТАЕТ** — 192.168.43.1 на wlan0, `onStarted` с задержкой ~10 с (норма для EMUI). Ожидание «часто не работает» не подтвердилось.
4. **Huawei: входящий TCP к приложению не доходит** — TCP-handshake со стороны пира завершается (клиент пишет initiation без ошибок), но accept-цикл приложения соединение не видит; при этом loopback-accept через `adb forward` работает штатно (accept + handler fire). Похоже на stealth-SYN firewall уровня ядра/ven­dor. Вердикт зафиксирован как «вход заблокирован вендором, требуется whitelist в Phone Manager» — подтвердить ручным тогглом на устройстве.
5. **Huawei: процесс убивается EMUI каждые ~3–4 минуты в фоне** — deviceidle-whitelist НЕ спасает. Спасает foreground-activity + включённый экран (`stay_on_while_plugged_in=7`). Для прогонов: держать MainActivity на экране.
6. **Mi 9T и Redmi 5 (MIUI): `adb install` отклонён — INSTALL_FAILED_USER_RESTRICTED** — нужен ручной тоггл «Установка через USB» в Developer options на каждом устройстве. До тоггла вне парка.
7. **Инструментарий прогона (в коде, debuggable-only):** debug-broadcast `app.mush.debug.CMD` (join/send/flood/decoy/start/lan), логи invite/credentials/onStarted/onFailed/accepted connection/handshake/message/error в logcat под тегом `mush`. Локальный только скрипт-клиент вместо QR.

Вывод Phase 0 на текущем парке: **прямой device-to-device TCP заблокирован вендорами на обоих доступных устройствах.** Следующие шаги: (а) ручные whitelist-тогглы на POCO/Huawei (Security app / Phone Manager) и «Установка через USB» на Mi 9T/Redmi 5; (б) owner-тест на Samsung A36 при приезде (Global-прошивки обычно не блокируют inbound); (в) если вендорские блоки снять нельзя — пересмотр транспортной модели Phase 0 (outbound-only роли / рандеву через третий узел) как отдельное архитектурное решение.

## 4. Phase 1 — после приезда Samsung

Samsung = Owner/Reference. POCO переходит в Stress Client.

### 4.1. Pre-flight

- [ ] Samsung: `getprop ro.csc.sales_code` → EUX/SER/DBT (Global). Если MTS/SBL/BIL → hotspot может быть заблокирован.
- [ ] Кабели: оригинальные или UGREEN/Baseus 3A, по одному на устройство. Дешёвые кабели + 5 портов на хабе = flaky ADB.
- [ ] `adb devices -l` — все 5 устройств в статусе device. Один offline или unauthorized = чинить кабель/порт, потом начинать прогон.
- [ ] Battery optimization off на всех устройствах.
- [ ] Logcat фильтр: `adb logcat -s mush:D AndroidRuntime:E *:S`.

### 4.2. Пайплайн (14 дней)

Дни 1–2 — Transport (Milestone 2 на Samsung):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 1.1 | Samsung (owner) | startLocalOnlyHotspot → onStarted, IP определён | IP 192.168.x.1 |
| 1.2 | Samsung + POCO | POCO коннектится к Samsung hotspot, TCP connect | Пакеты ходят |
| 1.3 | Samsung + POCO | Handshake, session key, session ID совпадает | Логи обеих сторон |

Дни 3–4 — A' критерии 1–2:

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 2.1 | Samsung + POCO | QR-контакт, /send hello | Сообщение у обоих |
| 2.2 | Samsung + POCO | Airplane mode на обоих, QR-контакт | Handshake проходит |

День 5 — LAN fallback (критерий 3):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 3.1 | Samsung + Huawei | Deny hotspot → LAN mode → IP из QR | Сообщение доходит |

Дни 6–8 — Group chat (критерий 4 + MS5):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 4.1 | Samsung + POCO + Mi 9T + Huawei | 4 человека, QR-контакт | CRDT идентичен |
| 4.2 | Все 5 (добавить Redmi 5) | 5 человек, stress merge | История идентична |
| 4.3 | Все 6 (если второй Samsung) | 6 человек | Consistency |

Дни 9–10 — Sensors (критерии 5–7):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 5.1 | POCO | Debug /flood 50 → S1 alert | Alert в UI |
| 5.2 | POCO | Debug /decoy → S7 alert | Alert в UI |
| 5.3 | POCO | /flood 150 → backpressure | Буфер ≤ 100 |

Дни 11–12 — Soak (критерий 8) + MS5 (A' Done) + метрики:

MS5 = точка закрытия A'. Всё, что за ней — уже этап B.

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 8.1 | POCO | 30 мин в фоне, dumpsys activity services | isForeground=true, PID не менялся |
| 8.2 | Samsung | 30 мин monkey test | Нет крашей |
| 8.3 | Samsung + POCO | verify → re-handshake → Verified сохранился | Поведенческий тест |
| M.1 | Все | Battery drain: 1 час группового чата | График разряда |
| M.2 | Все | Hotspot IP лог: Samsung, POCO, Mi 9T, Huawei, Redmi | Таблица vendor-IP |
| M.3 | Все | Финальный демо-прогон: 4 телефона, 15 мин | Видео/скриншоты |

—— MS5 (A' Done) закрывается в конце дня 12. ——

День 13 — MS6 (B-start). Не блокирует A'.

| # | Устройства | Задача | PASS |
|---|---|---|---|
| KS.1 | Все | Keystore-обёртка DEK на реальном hardware-backed | Vault открывается |
| KS.2 | Samsung + POCO | StrongBox vs TEE: разница в latency | Замер ms |
| KS.3 | Huawei | Kirin TEE compatibility | Fallback к software если fail |

День 14 — Буфер + persistence (ручной скрипт R.1):

| # | Задача | PASS / Команда |
|---|---|---|
| R.1 | Restart-persistence: force-stop + restart на 3 устройствах | Проверено |

Команды (10 минут на всё):

```bash
adb -s <serial> shell am force-stop app.mush
adb -s <serial> shell am start -n app.mush/.MainActivity
sleep 3
adb -s <serial> logcat -d -s mush:D | grep "node state imported"
```

Ожидаемый вывод: identity та же, Vault открылся.
Автоматизация (connectedAndroidTest) — отдельной задачей в CI после MS5, не блокирует hardware-прогон.

## 5. Критерии приёмки — сводка

### 5.1. A' базовые (8 из MUSH-A'-SPEC §10)

| # | Критерий | Устройства | Фаза |
|---|---|---|---|
| 1 | Два телефона обмениваются | Samsung + POCO | Phase 1, дни 1–4 |
| 2 | QR без интернета | Samsung + POCO | Phase 1, день 4 |
| 3 | LAN fallback | Samsung + Huawei | Phase 1, день 5 |
| 4 | Групповой чат 4+ | Samsung + POCO + Mi 9T + Huawei | Phase 1, дни 6–8 |
| 5 | S1 alert | POCO | Phase 1, день 9 |
| 6 | S7 decoy | POCO | Phase 1, день 9 |
| 7 | Backpressure | POCO | Phase 1, день 10 |
| 8 | 30 мин без крашей | Все по очереди | Phase 1, дни 11–12 |

### 5.2. Этап A (Ratchet) на железе

| # | Критерий | PASS |
|---|---|---|
| A.1 | Session ID совпадает на обеих сторонах handshake | Логи |
| A.2 | exportState → kill → importState → обмен продолжается | Логи |
| A.3 | FS: перехваченные фреймы до компрометации не расшифровываются | Логи |
| A.4 | Activation folding: потеря активации → обмен возобновляется | Логи |

### 5.3. Этап B (Vault, Identicon, Safety Number) на железе

| # | Критерий | PASS |
|---|---|---|
| B.1 | Identity seed + pre-keys + сессии переживают force-stop (проверяется через R.1) | Перезапуск → та же identity |
| B.2 | Identicon совпадает Rust ↔ Kotlin (golden vectors) | JVM-тест PASSED |
| B.3 | Safety Number 60 цифр, совпадает у обеих сторон | Сверка глазами |
| B.4 | verify → force-stop → restart → Verified сохранился | Поведенческий тест |
| B.5 | verify → re-handshake → Verified не сбрасывается | Поведенческий тест |
| B.6 | «Не совпадает» → VerificationFailed, красный ✗, снимается повторной сверкой | UI |
| B.7 | Keystore-обёртка DEK вместо Plaintext | Vault открывается |

### 5.4. minSdk boundary

| # | Устройство | PASS |
|---|---|---|
| S.1 | Redmi 5 (API 27) | Устанавливается, работает как клиент |
| S.2 | Redmi 5, если API < 26 | INSTALL_FAILED_OLDER_SDK — записать в протокол |

## 6. Метрики для заполнения

### 6.1. Vendor-таблица

| Device | Android | API | Hotspot IP | Hotspot works? | LAN fallback? | Owner OK? |
|---|---|---|---|---|---|---|
| Samsung A36 | 15 | 35 | _______ | Y / N | — | Y / N |
| POCO M6 Pro | 16 | 36 | _______ | Y / N | Y / N | Y / N |
| Xiaomi Mi 9T | 11 | 30 | _______ | Y / N | Y / N | Y / N |
| Huawei STK-LX1 | 10 | 29 | _______ | Y / N | Y / N | Y / N |
| Redmi 5 | 8.1 | 27 | — (client) | — | Y / N | — |

### 6.2. Метрики производительности

| Метрика | Как собрать | Целевое |
|---|---|---|
| Cold start | adb logcat → Activity.onCreate → первый UI | < 2 сек |
| Memory footprint | Android Studio Profiler | < 80 MB |
| Battery drain (1 час чата) | Battery Historian | < 5%/час |
| APK size | ls -la app-release.apk | < 20 MB |
| Foreground service lifetime | dumpsys activity services | > 30 мин |
| Handshake latency | Log timestamp первого X3DH до session ID | < 500 мс |

### 6.3. Logcat-фильтр (одинаковый для всех)

```bash
adb logcat -s mush:D mush_jni:D AndroidRuntime:E *:S
```

## 7. Отладка — что делать при падении

| Где | Что проверить | Вердикт |
|---|---|---|
| POCO hotspot | onFailed reason=N в logcat | N=3 → rate-limit, fallback owner №2 |
| POCO battery kill | Settings → Battery → No restrictions | Если выключено → включить, перетестировать |
| Samsung hotspot | getprop ro.csc.sales_code | MTS/SBL/BIL → операторский, риск блокировки |
| Huawei hotspot | Работает ли вообще | Ожидаемо fail → LAN fallback |
| Mi 9T handshake | Logcat, сравнить с Samsung | Если Samsung OK, Mi 9T нет → vendor quirk |
| Все сразу | Native.init проходит? JNI жив? | Блокер → чинить bridge, не devices |
| Redmi 5 | getprop ro.build.version.sdk | 27 → in scope, 25 → out |
| CRDT merge расходится | Сравнить истории на 3+ устройствах | Баг в ChatStore::merge |

Стоп-условия (остановить прогон, чинить сначала):

1. Native.init возвращает 0 или падает.
2. Listener на 127.0.0.1, не на 0.0.0.0.
3. Handshake не доходит ни на одном устройстве.
4. APK не устанавливается на ≥2 устройствах подряд.
5. MS6 only: SQLCipher не открывает БД на hardware-backed Keystore (проверяется в день 13, не раньше).

## 8. Артефакты после прогона

В репо `mush/docs/hardware-tests/`:

```text
mush/docs/hardware-tests/
├── README.md                        # сводка прогона
├── vendor-table.md                  # заполненная таблица §6.1
├── metrics.md                       # метрики §6.2
├── hardware-verify-<ts>.log         # сырой лог hardware-check.sh
├── screenshots/
│   ├── phase0-poco-owner.png
│   ├── phase1-samsung-qr.png
│   ├── group-chat-4.png
│   ├── s1-alert.png
│   ├── s7-decoy.png
│   └── safety-number-verify.png
└── failures/                        # если были
    └── <date>-<issue>.log
```

После прогона — обновить (см. §10.3).

## 9. Что НЕ входит в план

| Не входит | Почему | Когда |
|---|---|---|
| iOS тесты | Отдельный проект, Swift + JNI | Этап B-status |
| Android Go (2 ГБ) | Не extreme low-end для A' | Этап C |
| Планшеты | Другой форм-фактор | Этап D |
| Telegram/Signal bridge | Требует отдельной архитектуры | Этап B-status |
| 100+ узлов mesh | Не покрывается 6 устройствами | Этап C стресс |
| Vironet эволюция | Нет данных для эволюции на 6 устройствах | После этапа C |

## 10. Definition of Done

### 10.1. MS5 (A' Done) — закрывается в день 12

- [ ] Все 8 критериев A' §5.1 — PASS.
- [ ] Групповой чат 3–6 устройств — PASS.
- [ ] Sensors S1 + S7 — PASS на POCO.
- [ ] Soak 30 мин (критерий 8) — PASS на POCO + Samsung.
- [ ] Метрики M.1 (battery), M.2 (vendor-IP), M.3 (демо) — собраны.
- [ ] Демо-прогон 15 мин — записан (видео/скриншоты).

MS5 = точка закрытия A'. Это финальный барьер Phase 1.

### 10.2. MS6 (B-start) — день 13, НЕ блокирует A'

- [ ] Keystore-обёртка DEK на hardware-backed (KS.1).
- [ ] StrongBox vs TEE latency (KS.2).
- [ ] Kirin TEE fallback (KS.3).

MS6 — первый шаг этапа B. Начинается после закрытия MS5, но не входит в A'.

### 10.3. Тест-план считается закрытым, когда

- [ ] MS5 (A' Done) — закрыт.
- [ ] MS6 (B-start) — начат (не обязательно закрыт).
- [ ] minSdk boundary §5.4 — определён (in/out).
- [ ] Vendor-таблица §6.1 заполнена.
- [ ] Метрики §6.2 собраны.
- [ ] Артефакты §8 в репо.
- [ ] Обновлены три файла явно:
  - [ ] MUSH-A'-SPEC.md §8 — критерии с реальными фактами из прогона.
  - [ ] MUSH-HARDWARE-TEST-PLAN.md — MS5 и MS6 помечены закрытыми, дата прогона.
  - [ ] MUSH-TEST-PLAN.md — Phase 0 / Phase 1 помечены как закрытые, ссылка на артефакты §8.
- [ ] CI зелёный на финальном коммите.

Когда DoD закрыт — этап A' Android считается завершённым. Следующий шаг: этап B (Vault, Identicon, Safety Number) → этап C (Onion, BLE, MAC rotation).

---

MUSH-HARDWARE-TEST-PLAN v1.2.0 — финальный. Phase 0 стартует на текущем парке. Phase 1 — после Samsung. MS5 (день 12) = закрытие A'. MS6 (день 13) = этап B (Keystore). День 14 = буфер + persistence.
