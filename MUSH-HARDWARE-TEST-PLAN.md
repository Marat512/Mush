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
| POCO M3 | 7536eb550221 | 31 | 12, MIUI 14.0.2 EEA | arm64-v8a | Owner №3 (inbound НЕ блокирован, §3.5 п.17) |
| Huawei STK-LX1 | MVS4C20509005466 | 29 | 10, EMUI | arm64-v8a | LAN fallback / Client (в группе 3 узлов — п.19г) |
| Эмулятор | emulator-5554 | 30 | 11 | x86_64 | Rehearsal |
| iPhone 16 Pro Max | — | — | — | — | Out of scope |
| Samsung (1 шт. в пути) | — | 35 | 15 | arm64-v8a | Второй экземпляр (опционально) |

Покрытие API: 29 → 30 → 31 → 35 (в пути) → 36. Redmi 5 (API 27) в парке отсутствует — minSdk boundary непокрыт (§5.4, §10.3), точка 27 по нему снята из §2.
Покрытие вендоров: MIUI, HyperOS, EMUI, OneUI (в пути), AOSP (эмулятор).
Покрытие чипсетов: Snapdragon (POCO M3/M6, Mi 9T), Kirin (Huawei), Exynos (Samsung).

## 3. Phase 0 — закрыта 14.10, ждём Samsung для Phase 1

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

**Дополнение (вечер 13.10, прогон после тогглов):**

8. **Принцип тестирования Mush (DTN): успех на одной стороне не доказывает ничего про вторую.** Причина фундаментальна: X3DH по дизайну асинхронен — initiator не ждёт ответа, потому что responder в DTN может быть offline часами; локальный эмит HandshakeComplete — фича, не баг. Значит, симптом «у joiner'а Handshake complete, у owner'а тишина» = блок inbound на owner'е, а не успех. **Ассерт любого hardware-теста — на обеих сторонах** (accepted connection + HandshakeComplete у обоих, либо доставка `message from`). В групповом чате (дни 6–8) тот же принцип: «сообщение отправлено» у одного участника не доказывает доставку остальным — ассерт по истории всех узлов.
9. **Тогглы пользователя (13.10 вечер):** Huawei Phone Manager (автозапуск/фон) — выживание процесса починено (15+ мин в фоне против 3–4), **но inbound TCP не разблокирован**. POCO (HyperOS Security) — inbound не разблокирован (SYN drop сохраняется в foreground и с включённым экраном). Точные вендорные пути для inbound-allowlist не найдены (веб-поиск не дал подтверждённого решения) — требуется ручной поиск на устройстве либо Samsung как owner.
10. **LAN-фallback (0.3) на парке = FAIL по той же причине.** Проверен путь через домашний роутер (POCO ↔ Huawei, оба на STA 192.168.31.x): kernel-handshake завершается (TIME_WAIT у клиента), accept у приложения не fire'ится — **stealth-блок EMUI глобален по интерфейсам, не специфичен для softap**. LAN-режим вендорский firewall не обходит. Для парка вывод: ни hotspot, ни LAN не дают прямого TCP между HyperOS/EMUI-устройствами.
11. **Redmi 5 (0.4): установка заблокирована** — INSTALL_FAILED_USER_RESTRICTED (тот же MIUI-тоггл «Установка через USB», пользователь не уверен, что включён). API 27 подтверждён → in scope как клиент, как только APK установится (ручная установка с флешки обходит ограничение).

**Дополнение 2 (вечер 13.10, прорыв):**

12. **0.2 PASS на железе: Huawei (EMUI 10, arm64) ↔ Redmi 5 (MIUI 8.1, API 27).** Ручная установка APK на Mi 9T и Redmi 5 прошла; Redmi 5 как owner: `accepted connection` + HandshakeComplete на обеих сторонах, двусторонний /send доставлен (`hello` туда, `privet` обратно). Вся цепочка X3DH + Double Ratchet + CRDT-доставка работает на реальном ARM-железе двух вендоров.
13. **Вендорский inbound-блок — привилегия НОВЫХ оболочек.** Redmi 5 на MIUI 8.1 (2018, API 27) входящие TCP к приложению ПРИНИМАЕТ — stealth/hard-блоков нет. Старый парк выстраивается: **owner = Redmi 5** (или Samsung при приезде), HyperOS/EMUI-устройства — клиентами (их outbound работает штатно).
14. minSdk boundary: API 27 — полный cold start без крашей (hotspot, listener, invite, foreground-service). Самая нижняя вилка поддержки (26–27) на железе подтверждена.
15. **0.5 (группа 3 узла) заблокирован багом runtime на arm32/arm64-железе: на Redmi 5 у tokio-runtime ровно ОДИН worker-поток вместо двух** (`worker_threads(2)` в runtime.rs; 20 потоков процесса, один `mush-jni`; panic'ов/SIGABRT в полном logcat нет — воркер не родился или умер бесшумно). Симптом: первый пир коннектится и handshake'ится штатно (в т.ч. по LAN, без вендорных блоков), **второе входящее соединение завершается на kernel-уровне (TIME_WAIT у клиента), но accept-цикл его не забирает** — классическое голодание единственного воркера. Воспроизводится стабильно (3 прогона). Гипотезы к проверке: (а) воркер умирает на первом же spawn без вывода (panic-hook перехвачен?); (б) Builder на устройстве сжимает пул (не подтверждено — на Huawei/POCO воркеров 2); (в) flush/read путь блокирует единственный воркер sync-секцией. Следующий шаг: instrument'ить runtime.rs (лог на старт/панику каждого воркера) + проверить `std::thread::available_parallelism()` на устройстве. Задача delegable: код + цикл перепроверки на железе. **0.2 при этом не затронут (один пир работает штатно).**

16. **Root cause п.15 найден и зафиксирован (коммит `9ade452`, 13.10 вечер): reentrant `block_on` убивал воркер.** JNI-callback'и (onHandshakeComplete/onMessage) исполняются НА воркере tokio (dispatcher-задача, §5.3); Kotlin синхронно дёргал `Native.exportState` (autosave по throttle) → `runtime().block_on` внутри runtime-контекста → panic «Cannot start a runtime from within a runtime» → воркер умирал бесшумно (Rust-stderr в logcat не идёт; плюс ART переименовывает attach'енный поток в `Thread-N`, поэтому comm-подсчёт «1 поток mush-jni» занижал — воркеров фактически стало 1). Фикс: `block_on_reentrant` (из runtime-контекста future исполняется на scoped-thread вне контекста; все 11 JNI-входов переведены), panic-hook дублирует payload+location в tracing/logcat, `on_thread_start/stop` логи, `available_parallelism` в лог при построении runtime. Регресс-тесты: `block_on_reentrant_inside/outside_runtime_context` (14 тестов jni, 100 core — все зелёные). **Гипотезы (а)–(в) из п.15 закрыты:** воркер не «не родился» и не «сжат пулом» — Builder гарантирует 2 воркера; воркер умирал на reentrant-пути. **Статус: ждёт перепроверки на железе** (свежий APK с фиксом). Чеклист прогона 0.5 на Redmi 5: (1) в logcat `mush-jni: runtime worker up` — РОВНО 2 строки; (2) `mush-jni: runtime worker ОСТАНОВЛЕН` и `PANIC в потоке` — ОТСУТСТВУЮТ; (3) второй inbound-пир: `accepted connection from` + HandshakeComplete у owner'а; (4) 0.5-сценарий: 3 узла, история идентична на всех. Если (1) дал 2, а (3) всё равно FAIL — поднимать гипотезу (в) (sync-секция во flush/read пути блокирует оба воркера) как следующий кандидат.

17. **Новое устройство в парке: POCO M3 (14.10) — inbound-проба §4.1 и 0.2-style handshake PASS, теперь owner №3.** Спека: M2010J19CG / citrus, Snapdragon 662 (bengal), 4 ГБ RAM, 64 ГБ storage (~17 ГБ free), 1080×2340 @440dpi, Android 12 (API 31), MIUI 14.0.2 EEA (V14.0.2.0.SJFEUXM, security patch 2023-04-01), arm64-v8a, serial `7536eb550221`. Прогон (оба устройства на LAN 192.168.31.x, свежий APK с фиксом `9ade452`): (1) **inbound-проба PASS** — raw TCP с Mi 9T: `accepted connection from 192.168.31.197` в logcat POCO; (2) **0.2-style handshake PASS на обеих сторонах** — `Handshake complete... Session key established` у POCO и у Mi 9T, `Group joined`; (3) доставка: `message from=CWu+Sj0w/0vJ text=privet iz testa` у POCO. (4) Рантайм с фиксом: **2 воркера `mush-jni` на обоих устройствах**, `runtime worker ОСТАНОВЛЕН`/`PANIC` отсутствуют — косвенное подтверждение п.16 на железе (частичное: полный чеклист п.16 — прогон 0.5 на Redmi 5, второй inbound-пир). **Ключевой вывод: MIUI 14 (Android 12) входящий TCP к приложению НЕ блокирует** — вендорский блок inbound из §3.6 пока специфичен для HyperOS 3.0 (hard drop) и EMUI 10 (stealth-SYN); MIUI 8.1 (п.13) и MIUI 14 (п.17) его не имеют. Гипотеза уточнена: блок — свойство новых оболочек Xiaomi (HyperOS), не MIUI как такового. Побочные замечания для прогонов: (а) `adb install` требует тоггла «Установка через USB» и клика на диалоге (известно, п.6); (б) инвайт после `cmd start` несёт hotspot-адрес — для LAN-прогонов слать `cmd lan`, дождаться строки `lan mode: advertised <LAN-IP>:<port>`, потом `cmd reinvite`; (в) `am start-foreground-service` на старом APK отклонён (service не exported) — обновить APK до свежего, после `am start MainActivity` сервис поднимается сам. **Следующий шаг: 0.5 (группа 3 узла) теперь разблокирована без Samsung** — owner = POCO M3 (arm64, MIUI 14), клиенты = Mi 9T + Huawei/Redmi 5.

18. **Phase 0.5 (группа 3 узла по LAN) и Phase 0.3 (прямой pairwise) — PASS (14.10, отчёт `docs/hardware-tests/phase-0.5-report.md`).** Схема: owner = POCO M3 (MIUI 14), client-1 = Mi 9T (MIUI 12), client-2 = **POCO M6 Pro (HyperOS 3.0)** вместо неподключённого Huawei. Все на роутере 192.168.31.x, логи streamed host-side. Результаты: оба join'а — `accepted connection` + Handshake complete на обеих сторонах (одна группа `group-81aa69`); все три отправителя (Mi 9T, M6, owner) доставлены обоим остальным узлам — **relay-перешифровка через owner-посредник впервые подтверждена на железе** (сообщение Mi 9T получено M6 и наоборот, pairwise-каналы перешифровываются на owner'е); состав истории идентичен на трёх узлах; `runtime worker up` ×2 на каждом, 0 PANIC, 0 ОСТАНОВЛЕН (полный чеклист п.16 на новом устройстве — закрыт для MIUI 14; на Redmi 5 перепроверка остаётся открытой). Phase 0.3: M6 → Mi 9T напрямую без owner'а (Mi 9T listener, handshake + доставка `direct-m6-to-mi9t`) — LAN-путь без центрального узла работает. **Ключевые находки прогона:** (а) **на M6 Pro был активен VPN NekoBox (`moe.nb4a`), `bypassable=false`, uid 0–99999** — весь трафик приложения уходил в tun0, TCP к LAN-адресу owner'а мёртв без ошибок (join_peer вис в SYN-retransmit; initiator при этом эмитит локальный `Handshake complete` — ровно ловушка §3.6). После `am force-stop moe.nb4a` join прошёл мгновенно. **Следствие: вывод §3.6 «HyperOS hard drop» получен на устройстве с VPN и подлежит ревизии** — inbound-пробу M6 Pro перепроверить с выключенным VPN до Phase 1 (скрипт `scripts/inbound-probe.sh`); (б) MIUI автоматически переподключается к запомненным AndroidShare-hotspot'ам — клиент уходил с роутера («Network is unreachable», os error 101): перед join'ом проверять `ip route` на КАЖДОМ устройстве; (в) M6 Pro при рестарте приложения сам поднимал hotspot (auto-resume дня 8) — его AP перехватывала клиентов: на прогонах сразу `cmd lan` на всех не-owner устройствах; (г) метрика «воркеры по comm» недостоверна на MIUI 14 (ART переименовывает attach'енный поток) — надёжно только строки `worker up`/`ОСТАНОВЛЕН` в streamed logcat; (д) logcat-буфер POCO M3 крошечный (события минутной давности ротированы) — прогоны только с host-side стримом; (е) debug-команды `send`/`lan`/`start`/`flood`/`decoy` не логируют приём — молчание лога ≠ зависание receiver'а (ложная тревога «wedge owner'а» по пути, разобрана в отчёте).

19. **Ревизия §3.6 — переворот таблицы (14.10, второй заход): вендорский inbound-блок НЕ подтверждён ни на одном устройстве.** Прогоны: (а) **M6 Pro (HyperOS 3.0) с VPN off — inbound-проба PASS** (`accepted connection` на `inbound-probe.sh`, ~10 с) — «HyperOS hard drop» был артефактом VPN NekoBox; (б) **Huawei (EMUI 10) — inbound-проба PASS** без всяких whitelist-тогглов — «EMUI stealth-SYN» не воспроизвёлся (в прогоне 13.10 на одном из устройств парный трафик, видимо, тоже шёл через VPN/иную обходную сеть; точную причину наблюдения 13.10 установить нельзя, симптом не воспроизводится); (в) **Mi 9T (MIUI 12) — inbound-проба PASS** (принял соединение даже из hotspot-сегмента); (г) **Huawei в группе 3 узлов (EMUI-клиент закрыт)**: owner POCO M3 + Mi 9T + Huawei — оба join'а с ассертами обеих сторон, relay Mi 9T ↔ Huawei через owner в обе стороны, все доставки, `worker up` ×2 на всех трёх, 0 PANIC; (д) **Hotspot на POCO M3 (MIUI 14) — owner без LAN-костыля**: `onStarted` 192.168.246.182 на отдельном интерфейсе wlan1 при живом STA wlan0 (конCURRENT, MIUI позволяет), креды (`ssid=AndroidShare_NNNN pass=...`) пишутся в logcat, клиент подключился через `cmd wifi connect-network` → join с ассертами обеих сторон + доставка в обе стороны. **Выводы:** «вендорский блок inbound» 13.10 был артефактом сетевой среды (прежде всего VPN с `bypassable=false`), а не политикой прошивок; MUSH.md §5.0 переписан (снято ограничение «владелец группы», оставлена оговорка про VPN). **Новый функциональный баг (не блокер, в трекинг): повторный join к рестартовавшему owner'у** — у клиента остаётся старая непустая группа + новая, `resolve_group("default")` требует ровно одну группу → send в default падает с `unknown group 'default'` (transport.rs:536–547). Сценарий: owner перезагрузил телефон → все клиенты не могут писать до ручного выбора группы/перезапуска. Фикс-кандидаты: рабочая группа = последняя joined, или send по умолчанию в последнюю активную. **Прогонные заметки:** `cmd wifi connect` на MIUI 12 падает (Binder) — использовать `cmd wifi connect-network <SSID> wpa2 <pass>`; после рестарта приложения клиент MIUI уходит с запомненного hotspot'а — переподключать принудительно; **не экспортировать `ANDROID_SDK_HOME` при gradle-сборке** — AGP кладёт/генерирует debug.keystore в `$ANDROID_SDK_HOME/.android`, подпись APK расходится с устройствами → INSTALL_FAILED_UPDATE_INCOMPATIBLE (14.10: случайно воспроизведено и разобрано).

20. **Баг п.19 (повторный join) — ЗАКРЫТ 14.10 (фикс + регресс-тест + перепроверка на железе).** Фикс: `Node::last_joined: Option<GroupId>` — устанавливается в `join_peer` (обе ветки, Ratchet и Legacy), `resolve_group("" / "default")` возвращает последнюю joined, fallback — прежнее поведение «ровно одна группа» для owner'а, не джойнившегося никуда (transport.rs). Не persist'ится (NodeState групп не хранит — согласовано). Регресс-тест `resolve_default_prefers_last_joined_group` (101 тест core, все зелёные). **Перепроверка на железе** (свежий APK на POCO M3 + Mi 9T): join → send OK (group-de9893) → рестарт owner'а → повторный join (group-0c9bb2, у клиента две группы) → send `msg-after-rejoin` **доставлен новому owner'у**, 0 ошибок «unknown group». Сценарий «owner перезагрузил телефон» теперь прозрачен для клиентов. **Huawei 0.1 / inbound-проба повторно зафиксирована с VPN off на обоих устройствах** (Huawei: tun0=0, VPN CONNECTED=0; POCO M3: tun0=0): `inbound-probe.sh` PASS — **EMUI остаётся в «works» (Y), не TBD**. Попутно в §6.1: Redmi 5 убран из таблицы (не в парке), §2 обновлён, покрытие API 29+31+30+36 (+35 в пути), minSdk boundary (§5.4, §10.3) помечен непокрытым. **Phase 0 закрыта** — все задачи 0.1–0.5 выполнены или осознанно исключены (minSdk), ждём Samsung для Phase 1.

## 3.6. Vendor inbound blocking (устаревший раздел — см. п.19 §3.5)

**⚠️ РЕВИЗИЯ 14.10 (п.19 §3.5): «вендорский блок inbound» НЕ подтверждён.** Пробы `scripts/inbound-probe.sh` по всему парку с VPN off — PASS на всех устройствах (M6 Pro/HyperOS, Huawei/EMUI 10, Mi 9T/MIUI 12, POCO M3/MIUI 14, Redmi 5/MIUI 8.1 по п.13). Гипотеза 13.10 о вендорском firewall была артефактом сетевой среды — прежде всего VPN NekoBox (`bypassable=false`) на M6 Pro. Исторический текст раздела ниже сохранён как есть; таблица причин остаётся полезной как чеклист диагностики «inbound не работает»: (1) VPN на устройстве — **проверять первым**, (2) AP isolation, (3) сегменты. MUSH.md §5.0 переписан.

**Симптом на парке (13.10, опровергнуто 14.10):** оба устройства (POCO HyperOS 3.0, Huawei EMUI 10) роняют входящие TCP к приложению при работающем listener'е (loopback-accept через `adb forward` работает штатно). «SYN drop» — не один феномен, три разные причины с разными следствиями. **Конфаунд 14.10 (п.18а): вывод про HyperOS получен на M6 Pro с активным VPN NekoBox (`bypassable=false`)** — VPN перехватывал весь трафик приложения; «hard drop» требует перепроверки с VPN off (`scripts/inbound-probe.sh`).

| # | Причина | Что это | Следствие |
|---|---|---|---|
| 1 | Vendor firewall (app-level) | HyperOS/EMUI блокируют входящие для приложений без явного разрешения; тоггл существует, но не найден | Samsung Global, вероятно, не страдает; лечится whitelist'ом вендора |
| 2 | **AP isolation в startLocalOnlyHotspot** | На части Android клиенты локального hotspot'а не могут достучаться до TCP-сервисов самого AP-хоста — документированное поведение | **Samsung НЕ спасёт** — тот же API. Лечение: WifiConfiguration+reflection (deprecated, местами работает) или LAN-режим как основной, а не fallback |
| 3 | NAT между сегментами | Клиент и хост в разных подсетях — SYN дропает маршрутизатор | Лечится адресацией (оба в одном сегменте) |

**Различение — tcpdump на клиенте (один прогон, ~20 мин):** capture на owner'е (wlan0, порт listener'а) во время join. SYN приходит на интерфейс → блок уровня хоста (firewall #1). SYN не приходит → AP isolation (#2) или сегменты (#3). Результат определяет, нужна ли переработка транспорта: при #2 пересмотр обязателен до Phase 1.

**Результат различения (13.10 вечер, /proc/net/tcp вместо tcpdump — бинаря нет на обоих): причина №1 — host-level vendor firewall, у вендоров разные модели блока:**

| Направление | Наблюдение | Модель блока |
|---|---|---|
| POCO → Huawei (AP у Huawei) | Kernel-handshake ЗАВЕРШАЕТСЯ (клиент в TIME_WAIT), **accept у приложения не fire'ится** | EMUI: **stealth-SYN** — firewall отвечает на handshake сам, соединение устанавливается, приложению не доставляет |
| Huawei → POCO (AP у Huawei) | Клиент застревает в **SYN_SENT**, retransmit растёт, у owner'а только LISTEN-сокет | HyperOS: **hard drop** — SYN игнорируется молча, таймаут у клиента |

Следствия:
- **AP isolation (№2) и NAT (№3) ИСКЛЮЧЕНЫ** — handshake на AP-пути у EMUI доходит до kernel хоста ⇒ доставка client↔host через startLocalOnlyHotspot работает.
- **Общий принцип тестирования Mush: локальное «успешно» не доказывает удалённое состояние.** На враждебных сетях и в DTN любая локальная операция может лгать — ассерт всегда симметричный, на обеих сторонах:

  | Что проверяется | Локальное «успешно» | Ассерт |
  |---|---|---|
  | Handshake (Phase 0.2) | HandshakeComplete у initiator'а | accepted connection + HandshakeComplete у обоих |
  | Групповой чат (дни 6–8) | send вернул true / нет ошибки | сообщение видно на всех участниках |
  | Verify (дни 11–12) | MarkVerified сохранён локально | Verified на обеих сторонах + переживает restart |

  Частные проявления: HandshakeComplete у initiator'а не доказывает связность (X3DH асинхронен по дизайну); connect() успешен ≠ пир доступен (EMUI stealth-SYN отвечает на handshake сам — mush-инициатор верит, что пир доступен; HyperOS даёт честный таймаут).
- **Samsung — гипотеза, не факт.** Исключение AP isolation снимает главную тревогу, но Samsung — отдельный вендор со своими драйверами; на части прошивок AP isolation включён по умолчанию независимо от API. Первый прогон при приезде — inbound-проба §4.1, до всего остального; fail пробы = гипотеза опровергнута, думаем дальше.
- Лечение блока — только вендорский whitelist (точный путь не найден; bounded exploration 30 мин) или смена owner'а на устройство без блока.

Статус (устарело 14.10, см. п.19): ~~POCO/Huawei — блок зафиксирован на kernel-уровне, причина различена; Samsung A36 — гипотеза, проверка inbound-пробой §4.1 при приезде. Пользовательское ограничение зафиксировано в MUSH.md §5.0.~~ Актуально: вендорский блок inbound не подтверждён ни на одном устройстве парка (все пробы PASS с VPN off); Samsung при приезде всё равно проходит inbound-пробу §4.1 первым делом (плюс проверка AP isolation на его прошивке).

## 4. Phase 1 — после приезда Samsung

Samsung = Owner/Reference. **POCO M3** (MIUI 14) — подтверждённый owner-резерв (hotspot + LAN, п.17/19): держать под рукой на случай сюрпризов Samsung. **M6 Pro** (HyperOS) — Stress Client (inbound у неё работает, п.19а; hotspot owner'ом на ней не проверялся). Без Redmi 5 (не в парке, §5.4). Парк Phase 1: **Samsung, POCO M3, M6 Pro, Mi 9T, Huawei** — 5 устройств.

### 4.1. Pre-flight

- [ ] Samsung: `getprop ro.csc.sales_code` → EUX/SER/DBT (Global). Если MTS/SBL/BIL → hotspot может быть заблокирован.
- [ ] **Inbound-проба на КАЖДОМ устройстве парка до старта пайплайна** (VPN на пробуемом устройстве выключен — VPN с `bypassable=false` полностью имитирует вендорный блок, п.18а/19 §3.5): нода подняла listener → TCP-connect к ней с другого устройства (не через `adb forward` — он loopback и блок не показывает) → строка `accepted connection` в logcat обеих сторон. Нет строки = смотреть чеклист §3.6 (VPN → AP isolation → сегменты). Без этой пробы дни 1–2 уйдут на диагностику «handshake не доходит» вместо реальных тестов. **Готово как скрипт: `scripts/inbound-probe.sh <owner_serial> <client_serial>`** (п.18 §3.5). Дополнительно перед пробой: `ip route` на обоих устройствах (MIUI авто-джойнится к запомненным hotspot'ам, п.18б) и VPN off на probing-устройстве (п.18а).
- [ ] Кабели: оригинальные или UGREEN/Baseus 3A, по одному на устройство. Дешёвые кабели + 5 портов на хабе = flaky ADB.
- [ ] `adb devices -l` — все 5 устройств (Samsung, POCO M3, M6 Pro, Mi 9T, Huawei) в статусе device. Один offline или unauthorized = чинить кабель/порт, потом начинать прогон.
- [ ] Battery optimization off на всех устройствах.
- [ ] Logcat фильтр: `adb logcat -s mush:D AndroidRuntime:E *:S`.

### 4.2. Пайплайн (14 дней)

Дни 1–2 — Transport (Milestone 2 на Samsung):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 1.1 | Samsung (owner) | startLocalOnlyHotspot → onStarted, IP определён | IP 192.168.x.1 |
| 1.2 | Samsung + M6 Pro | M6 коннектится к Samsung hotspot, TCP connect | Пакеты ходят |
| 1.3 | Samsung + M6 Pro | Handshake, session key, session ID совпадает | Логи обеих сторон |
| 1.4 | POCO M3 (owner-резерв) | Та же тройка 1.1–1.3 на POCO M3 | PASS (регресс MIUI 14 уже зелёный, п.17/19) |

Дни 3–4 — A' критерии 1–2:

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 2.1 | Samsung + M6 Pro | QR-контакт, /send hello | Сообщение у обоих |
| 2.2 | Samsung + M6 Pro | Airplane mode на обоих, QR-контакт | Handshake проходит |

День 5 — LAN fallback (критерий 3):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 3.1 | Samsung + Huawei | Deny hotspot → LAN mode → IP из QR | Сообщение доходит |

Дни 6–8 — Group chat (критерий 4 + MS5):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 4.1 | Samsung + M6 Pro + Mi 9T + Huawei | 4 человека, QR-контакт | CRDT идентичен |
| 4.2 | Все 5 (добавить POCO M3) | 5 человек, stress merge | История идентична |
| 4.3 | Все 6 (если второй Samsung) | 6 человек | Consistency |

Дни 9–10 — Sensors (критерии 5–7):

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 5.1 | M6 Pro | Debug /flood 50 → S1 alert | Alert в UI |
| 5.2 | M6 Pro | Debug /decoy → S7 alert | Alert в UI |
| 5.3 | M6 Pro | /flood 150 → backpressure | Буфер ≤ 100 |

Дни 11–12 — Soak (критерий 8) + MS5 (A' Done) + метрики:

MS5 = точка закрытия A'. Всё, что за ней — уже этап B.

| # | Устройства | Задача | PASS |
|---|---|---|---|
| 8.1 | M6 Pro | 30 мин в фоне, dumpsys activity services | isForeground=true, PID не менялся |
| 8.2 | Samsung | 30 мин monkey test | Нет крашей |
| 8.3 | Samsung + M6 Pro | verify → re-handshake → Verified сохранился | Поведенческий тест |
| M.1 | Все | Battery drain: 1 час группового чата | График разряда |
| M.2 | Все | Hotspot IP лог: Samsung, POCO M3, M6 Pro, Mi 9T, Huawei | Таблица vendor-IP |
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

**ПОКРЫТ по эмулятору 14.10 (решение «зафиксировать boundary по эмулятору», п.20 §3.5):** AVD `mush-api27` (android-27, default, x86_64, Android 8.1.0). Прогон: установка app-debug.apk (minSdk 26) — **Success**; cold start — `node created`, `runtime worker up` ×2; `cmd start` → `listener started on 0.0.0.0:56849`; `cmd lan` → advertised 192.168.232.2:56849; `cmd reinvite` — invite сгенерирован; 0 ошибок, 0 FATAL. Boundary API 27 ≥ minSdk 26 подтверждён; реальное устройство API 27 (Redmi 5) в прогоне не участвовало — опционально вернуть в Phase 2.

| # | Устройство | PASS |
|---|---|---|
| S.1 | Эмулятор API 27 (AVD mush-api27) | ✅ 14.10: установка, нода, listener, invite — без крашей |
| S.2 | API < 26 | out of scope (minSdk = 26; физических устройств API < 26 в парке нет) |

**Инструментальная заметка (Phase 1):** эмулятор 37.2.12 + образ API ≤ 27 + NVIDIA/Windows падает во ВСЕХ GPU-режимах (host/swiftshader/swangle/guest — крэш в gfxstream/Vulkan). Рабочий режим для старых образов: **`emulator -avd mush-api27 -no-window -gpu off -no-audio -no-boot-anim`** (headless, софтверная отрисовка гостя). Для API ≥ 30 (mush-ms0) обычный оконный режим работает.

## 6. Метрики для заполнения

### 6.1. Vendor-таблица

| Device | Android | API | Hotspot IP | Hotspot works? | LAN fallback? | Owner OK? |
|---|---|---|---|---|---|---|
| Samsung A36 | 15 | 35 | _______ | Y / N | — | Y / N |
| POCO M6 Pro | 16 | 36 | _______ | Y (п.1 §3.5) | Y (14.10) | **Y (14.10, VPN off)** |
| Xiaomi Mi 9T | 11 | 30 | _______ | — | Y (14.10) | Y (14.10) |
| POCO M3 | 12 | 31 | 192.168.246.182 (wlan1, конCURRENT) | **Y (14.10)** | Y (14.10) | Y (14.10) |
| Huawei STK-LX1 | 10 | 29 | 192.168.43.1 (п.3 §3.5) | Y | Y (14.10) | **Y (14.10, VPN off на обеих — повторная фиксация)** |

Заполнено 14.10 (п.19 §3.5). **Предусловие всех проб: VPN на устройстве выключен** (NekoBox с `bypassable=false` полностью имитирует вендорный блок inbound). Пустых ячеек не осталось, кроме Samsung (в пути). Redmi 5 из таблицы убран — устройство не в парке (14.10), minSdk boundary непокрыт.

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
| Входящий TCP: дифференциальный тест | Узел A коннектится к узлу B: accept через `adb forward` ЕСТЬ, при прямом Wi-Fi connect'е — НЕТ (connect при этом завершается успешно) | Это НЕ баг Mush — вендорский firewall (нах. 2/4 §3.5). Не искать в коде: сначала **tcpdump на owner'е** (wlan0, порт listener'а) — различить firewall (#1) vs AP isolation (#2) vs сегменты (#3) по §3.6; при #1 — whitelist вендора, при #2 — пересмотр транспорта |
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
- [ ] minSdk boundary §5.4 — определён (in/out). **✅ 14.10 — закрыто по эмулятору API 27** (установка, нода, listener, invite без крашей; S.2 out of scope).
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
