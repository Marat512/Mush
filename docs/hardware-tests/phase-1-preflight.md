# Phase 1 — Pre-flight чеклист (одна страница)

Выполняется в день приезда Samsung, ДО старта пайплайна §4.2.
Полный текст: `MUSH-HARDWARE-TEST-PLAN.md` §4.1. Код не трогать до конца чеклиста.

Сериалы парка: Samsung — `<samsung>`, POCO M3 `7536eb550221`, M6 Pro `eezpcaoznjrs5xxs`,
Mi 9T `f740d46d`, Huawei `MVS4C20509005466`.

На этом хосте adb не в PATH: `export ADB=/c/dev/sdk/platform-tools/adb.exe`
(скрипт `scripts/inbound-probe.sh` понимает переменную `ADB`).

## 1. Подключение

```bash
"$ADB" devices -l    # ожидание: 5 устройств, все в статусе device
```

Один offline / unauthorized = чинить кабель или порт, потом начинать прогон.
Кабели: оригинальные или UGREEN/Baseus 3A, по одному на устройство.

## 2. Samsung CSC — роль owner'а

```bash
"$ADB" -s <samsung> shell getprop ro.csc.sales_code
```

EUX / SER / DBT (Global) → owner, идём дальше.
MTS / SBL / BIL (операторские) → hotspot может быть заблокирован: Samsung стартует
как клиент, owner на первые дни = POCO M3 (подтверждённый резерв, план §4).

## 3. VPN off и маршрут на всех устройствах

```bash
for s in <samsung> 7536eb550221 eezpcaoznjrs5xxs f740d46d MVS4C20509005466; do
  echo "== $s =="
  "$ADB" -s "$s" shell ip route | grep tun0    # ожидание: пусто
done
```

VPN с `bypassable=false` полностью имитирует вендорный блок inbound (п.18а §3.5) —
NekoBox на M6 Pro уже сжёг нам один день. Также проверить `ip route` целиком:
MIUI авто-джойнится к запомненным hotspot'ам и уползает с роутера (п.18б §3.5).
Ожидание на всех: default via роутер 192.168.31.x.

## 4. Inbound-проба на Samsung (обязательно до пайплайна)

```bash
"$ADB" -s <samsung> shell am start -n app.mush/.MainActivity   # APK должен стоять
bash scripts/inbound-probe.sh <samsung> 7536eb550221
```

PASS = строка `accepted connection from` в logcat обеих сторон.
FAIL = смотреть чеклист §3.6 плана (VPN → AP isolation → сегменты), повторить;
без пробы дни 1–2 уйдут на диагностику «handshake не доходит» вместо тестов.

## 5. Остальное

- [ ] Battery optimization off на всех 5 устройствах.
- [ ] Logcat-фильтр прогонов: `adb logcat -s mush:D AndroidRuntime:E *:S`
      (логи прогонов — host-side стримом, буферы девайсов 256 КБ ротируют за секунды).
- [ ] Свежий debug APK на Samsung: `mush/mush-android/app/build/outputs/apk/debug/app-debug.apk`
      (не экспортировать `ANDROID_SDK_HOME` при сборке — README, warning).

## Таблица прогона Phase 1 (заполнять по ходу)

| Шаг | Устройства | Проба/задача | PASS | Фиксация |
|---|---|---|---|---|
| P0 | Samsung | CSC + inbound-probe | ☐ | |
| P1 | Samsung + POCO M3 | §4.2 дни 1–2 (transport) | ☐ | |
| P2 | Samsung + M6 Pro | §4.2 дни 3–4 (A' 1–2) | ☐ | |
| P3 | Samsung + Huawei | §4.2 день 5 (LAN fallback) | ☐ | |
| P4 | 4–5 устройств | §4.2 дни 6–8 (group chat, MS5) | ☐ | |
| P5 | M6 Pro / Samsung | §4.2 дни 9–12 (sensors, soak, метрики) | ☐ | |
| P6 | Все | Финальный демо-прогон + отчёт | ☐ | |

Отчёт по шагам — по образцу `docs/hardware-tests/phase-0.5-report.md`,
логи — в `docs/hardware-tests/logs/`.
