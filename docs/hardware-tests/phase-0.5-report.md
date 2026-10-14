# Phase 0.5 — отчёт: группа 3 узла по LAN + прямой pairwise (Phase 0.3)

Дата: 2026-10-14. Ветка: `feat/a1-cli` (APK с фиксом `9ade452` reentrant block_on).
Сеть: домашний роутер 192.168.31.x (НЕ hotspot). Логи: `logs/phase-0.5-*.log` (streamed с устройств, host-side).

## Устройства

| Роль | Устройство | Serial | ОС |
|---|---|---|---|
| Owner (listener) | POCO M3 | 7536eb550221 | MIUI 14.0.2 EEA, Android 12 (API 31) |
| Client-1 | Xiaomi Mi 9T | f740d46d | MIUI 12, Android 11 (API 30) |
| Client-2 | POCO M6 Pro | eezpcaoznjrs5xxs | HyperOS 3.0, Android 16 (API 36) |

Замена: планировался Huawei STK-LX1 как client-2 — не был подключён. POCO M6 Pro
взята вместо (owner №1 по плану, но inbound HyperOS заблокирован — клиентом
работает; это же в итоге дало ключевую находку про VPN, см. ниже).

## Результаты

| Шаг | Действие | Owner (POCO M3) | Получатели | Вердикт |
|---|---|---|---|---|
| 1 | Mi 9T join → owner | accepted connection + Handshake complete | Mi 9T: Handshake complete + `Group joined: group-81aa69` | PASS (обе стороны) |
| 2 | M6 Pro join → owner | accepted connection from .18 + Handshake complete | M6: Handshake complete + `Group joined: group-81aa69` | PASS (обе стороны) |
| 3 | Mi 9T: `privet-ot-mi9t` | `message from=CWu+Sj0w/0vJ` | M6: `message from=GDwU8ojKD26o text=privet-ot-mi9t` | **PASS — relay-перешифровка Mi 9T → M6 через owner** |
| 4 | M6: `privet-ot-huawei` | `message from=mZovVEa9Rc7M` | Mi 9T: `message from=GDwU8ojKD26o text=privet-ot-huawei` | **PASS — relay-перешифровка M6 → Mi 9T через owner** |
| 5 | Owner: `privet-ot-poco` | — | Mi 9T и M6: `message from=GDwU8ojKD26o text=privet-ot-poco` | PASS |
| 6 | Сравнение историй | видит все 3 чужих сообщения | каждый клиент видит все 3 сообщения | PASS — состав истории идентичен |
| 7 | Воркеры/panic | `runtime worker up` ×2, 0 PANIC, 0 ОСТАНОВЛЕН | то же на Mi 9T и M6 | PASS |

Phase 0.3 (прямой pairwise без owner'а): M6 Pro → Mi 9T напрямую (Mi 9T в роли
listener'а, `accepted connection from 192.168.31.18`, Handshake на обеих сторонах,
та же группа `group-81aa69`, доставка `direct-m6-to-mi9t`). **PASS** — LAN-путь
работает без центрального узла (fallback-сценарий подтверждён).

Это первое подтверждение на реальном железе: X3DH + Double Ratchet pairwise-каналы,
групповая доставка через relay-owner с перешифровкой, CRDT-консистентность
истории на трёх узлах трёх разных вендорских прошивок (MIUI 14 / MIUI 12 / HyperOS 3.0).

## Ключевые находки (пошли в план §3.5 п.18)

1. **VPN NekoBox (moe.nb4a) на M6 Pro `bypassable=false`, uid-range 0–99999 перехватывал весь трафик приложения** — TCP к LAN-адресу owner'а мёртв (ни ошибки, ни connect: join_peer зависает в SYN-retransmit; initiator при этом честно эмитит локальный `Handshake complete` — ловушка X3DH §3.6). После `am force-stop moe.nb4a` — join прошёл мгновенно. **Прямое следствие: вывод §3.6 про «HyperOS hard drop» получен на устройстве с активным VPN и требует ревизии** — inbound-пробу для M6 Pro перепроверить с выключенным VPN до Phase 1.
2. **MIUI автоподключение к запомненным AndroidShare-hotspot'ам** — Mi 9T дважды уходил с роутера на чужой hotspot («Network is unreachable», os error 101). Прогонная процедура: проверять `ip route` клиента перед join'ом, не только owner'а.
3. **M6 Pro при рестарте приложения сам поднимал hotspot** (auto-resume startNetworking — фича дня 8): его AP перехватывала клиентов (п.2). На прогонах — сразу `cmd lan` на всех не-owner устройствах.
4. **Метрика «число воркеров по comm» недостоверна на MIUI 14** — ART после `attach_current_thread` переименовывает поток, comm-подсчёт занижает; ложная тревога «воркер умер» по пути. Надёжно: строки `runtime worker up`/`ОСТАНОВЛЕН` в logcat (streamed host-side — девайсный буфер мал и ротирует строки за секунды) или полный список TID с `/proc/<pid>/task/*/comm`.
5. **Девайсный logcat-буфер на POCO M3 крошечный** — события минутной давности уже ротированы; единственный надёжный способ — `adb logcat` в файл на хосте на всём протяжении прогона.
6. Отладочные broadcast-команды `send`/`lan`/`start`/`flood`/`decoy` **не логируют факт приёма** (логирует только `join`, `reinvite` через `invite=`). Молчание лога ≠ зависание receiver'а.

## Подтверждённые ранее ограничения

- Owner на MIUI 14 (POCO M3) принимает inbound TCP штатно (п.17) — повторно подтверждено двумя inbound-join'ами.
- HyperOS 3.0 (M6 Pro) как клиент: outbound join + приём/отправка через relay — работает.
- EMUI/Huawei в этом прогоне не участвовал; его клиентская роль подтверждена ранее (п.12, 13.10).

## Открытое перед Phase 1

- Inbound-проба M6 Pro с VPN выключенным (пересмотреть «HyperOS hard drop» §3.6).
- Huawei в группе 3+ узлов (заменён в этом прогоне).
- Hotspot-режим на POCO M3 (прогон был LAN-only).
- minSdk boundary (Redmi 5) — вне прогона, помечено в DoD.
