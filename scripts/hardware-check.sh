#!/usr/bin/env bash
# hardware-check.sh — автоматический harness hardware-чеклиста MUSH-B-SPEC §6.
# Заменяет ad-hoc ручной прогон: два устройства, install APK, старт нод,
# автопроверки по logcat, guided ручная часть (QR + /send), артефакт со сводкой.
#
# Использование:
#   HW_OWNER=<id> HW_CLIENT=<id> ./hardware-check.sh
#   HW_WAIT_SEC=30 ./hardware-check.sh        # окно сбора логов (default 20)
#   HW_PUBKEY_PATTERN='pubkey: ' ./hardware-check.sh
#   HW_SESSION_PATTERN='session: ' ./hardware-check.sh
#
# Репетиция на эмуляторе: x86_64-ABI — warning, не отказ. Одно устройство —
# честный выход на preflight (чеклист требует ДВА). Один эмулятор для обеих
# ролей можно эмулировать через HW_SINGLE_DEVICE_REHEARSAL=1 (loopback-режим:
# owner и client — одно устройство, сессионная сверка MANUAL).
#
# Код выхода: 0 — нет FAIL; 1 — есть FAIL / preflight не пройден; 2 — нет APK.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ADB="${HW_ADB:-/c/dev/sdk/platform-tools/adb.exe}"
WAIT_SEC="${HW_WAIT_SEC:-20}"
PUBKEY_PATTERN="${HW_PUBKEY_PATTERN:-pubkey: }"
SESSION_PATTERN="${HW_SESSION_PATTERN:-session: }"
APK="$SCRIPT_DIR/../mush/mush-android/app/build/outputs/apk/debug/app-debug.apk"

# Git Bash манглит /data/... в C:/Program Files/Git/... — отключаем конверсию.
export MSYS2_ARG_CONV_EXCL='*'

say() { printf '%s\n' "$*"; }
adb() { "$ADB" "$@"; }

PASS=0; FAIL=0; MANUAL=0
RESULTS=()

check() { # $1 = имя проверки, $2 = статус (PASS|FAIL|MANUAL), $3 = детали
    local name="$1" status="$2" detail="${3:-}"
    case "$status" in
        PASS) PASS=$((PASS+1));;
        FAIL) FAIL=$((FAIL+1));;
        MANUAL) MANUAL=$((MANUAL+1));;
    esac
    RESULTS+=("[$status] $name${detail:+ — $detail}")
    say "  [$status] $name${detail:+ — $detail}"
}

# ---------------------------------------------------------------------------
# 1. Preflight: adb + ровно два устройства + ABI.
# ---------------------------------------------------------------------------
say "== Preflight =="
if [ ! -x "$ADB" ] && ! command -v "$ADB" >/dev/null 2>&1; then
    say "ERROR: adb не найден: $ADB (переопредели через HW_ADB)"
    exit 1
fi

# shellcheck disable=SC2046
DEVICES=$(adb devices | awk 'NR>1 && $2=="device" {print $1}')
DEV_COUNT=$(printf '%s\n' "$DEVICES" | grep -c . || true)

OWNER="${HW_OWNER:-$(printf '%s\n' "$DEVICES" | sed -n 1p)}"
CLIENT="${HW_CLIENT:-$(printf '%s\n' "$DEVICES" | sed -n 2p)}"

if [ "$DEV_COUNT" -lt 2 ]; then
    if [ "${HW_SINGLE_DEVICE_REHEARSAL:-0}" = "1" ] && [ "$DEV_COUNT" -eq 1 ]; then
        say "WARNING: одно устройство, HW_SINGLE_DEVICE_REHEARSAL=1 — репетиция loopback"
        CLIENT="$OWNER"
    else
        say "ERROR: найдено устройств: $DEV_COUNT (нужно 2: owner + client)."
        say "       Подключи оба, разреши USB-debugging (status 'device', не 'unauthorized')."
        say "       Для репетиции на одном эмуляторе: HW_SINGLE_DEVICE_REHEARSAL=1"
        exit 1
    fi
fi
say "OWNER=$OWNER  CLIENT=$CLIENT"

for dev in OWNER CLIENT; do
    id="$(eval "echo \$$dev")"
    abi=$(adb -s "$id" shell getprop ro.product.cpu.abi 2>/dev/null | tr -d '\r')
    case "$abi" in
        arm64-v8a|armeabi-v7a)
            check "$dev ABI ($abi)" PASS ;;
        x86_64|x86)
            check "$dev ABI ($abi)" PASS "эмулятор: репетиция, на железе будет arm*" ;;
        *)
            check "$dev ABI" FAIL "неожиданный ABI: '$abi'" ;;
    esac
done

# ---------------------------------------------------------------------------
# 2. APK.
# ---------------------------------------------------------------------------
say "== APK =="
if [ ! -f "$APK" ]; then
    say "ERROR: APK не найден: $APK"
    say "       Сборка: cd mush/mush-android && JAVA_HOME='C:\\dev\\jdk-17.0.20.1+1' ./gradlew.bat assembleDebug"
    exit 2
fi
check "APK существует" PASS "$(basename "$APK")"

# ---------------------------------------------------------------------------
# 3. Install + старт на обоих.
# ---------------------------------------------------------------------------
say "== Install & start =="
# adb.exe — Windows-бинарь: Git Bash-путь /c/... ему непонятен, конвертируем.
APK_WIN="$(cygpath -w "$APK" 2>/dev/null || echo "$APK")"
for id in "$OWNER" "$CLIENT"; do
    uniq="$id"; [ "$id" = "$OWNER" ] && [ "$id" = "$CLIENT" ] && uniq="(loopback)"
    if adb -s "$id" install -r "$APK_WIN" >/dev/null 2>&1; then
        check "install $uniq" PASS
    else
        check "install $uniq" FAIL "adb install -r вернул ошибку"
    fi
done
# Подготовка: runtime permissions + имя в prefs. Без имени приложение
# остаётся на onboarding и сервис не стартует — автопроверки видели бы
# пустой лог. Debuggable APK: run-as позволяет писать shared_prefs.
say "== Подготовка устройств (permissions + prefs) =="
ROLE=OWNER
for id in "$OWNER" "$CLIENT"; do
    for perm in ACCESS_FINE_LOCATION ACCESS_COARSE_LOCATION CAMERA; do
        adb -s "$id" shell pm grant app.mush "android.permission.$perm" >/dev/null 2>&1 || true
    done
    name="Mush $ROLE"
    adb -s "$id" shell "run-as app.mush sh -c 'mkdir -p shared_prefs && printf \"<?xml version=\\\"1.0\\\" encoding=\\\"utf-8\\\"?><map><string name=\\\"name\\\">$name</string></map>\" > shared_prefs/mush_prefs.xml'" >/dev/null 2>&1 \
        && check "prefs seed $ROLE ($id)" PASS \
        || check "prefs seed $ROLE ($id)" MANUAL "run-as не сработал — пройди onboarding вручную"
    ROLE=CLIENT
done
for id in "$OWNER" "$CLIENT"; do
    # force-stop: install -r не перезапускает процесс, а 'node created'
    # пишется один раз при init — без рестарта в логе его не будет.
    adb -s "$id" shell am force-stop app.mush >/dev/null 2>&1 || true
    if adb -s "$id" shell am start -n app.mush/.MainActivity >/dev/null 2>&1; then
        check "am start $id" PASS
    else
        check "am start $id" FAIL "activity не стартовала"
    fi
done

# ---------------------------------------------------------------------------
# 4. Сбор логов: очистка, окно ожидания, dump.
# ---------------------------------------------------------------------------
say "== Log window: ${WAIT_SEC}s =="
LOGS_DIR="$(mktemp -d)"
for id in "$OWNER" "$CLIENT"; do
    adb -s "$id" logcat -c >/dev/null 2>&1 || true
done
sleep "$WAIT_SEC"
for id in "$OWNER" "$CLIENT"; do
    tag="$id"; [ "$id" = "$OWNER" ] && [ "$id" = "$CLIENT" ] && tag="loopback"
    adb -s "$id" logcat -d -s mush:V AndroidRuntime:E >"$LOGS_DIR/$tag.log" 2>&1 || true
done
say "логи: $LOGS_DIR"

# ---------------------------------------------------------------------------
# 5. Автопроверки по логам.
# ---------------------------------------------------------------------------
say "== Автопроверки =="
OWNER_TAG="$OWNER"; CLIENT_TAG="$CLIENT"
if [ "$OWNER" = "$CLIENT" ]; then OWNER_TAG="loopback"; CLIENT_TAG="loopback"; fi

for side in OWNER CLIENT; do
    tag="$(eval "echo \$${side}_TAG")"
    log="$LOGS_DIR/$tag.log"
    [ -f "$log" ] || { check "$side: лог собран" FAIL "$log отсутствует"; continue; }
    check "$side: лог собран" PASS "$(wc -l <"$log") строк"
    if grep -q "node created" "$log"; then
        check "$side: node created" PASS
    else
        check "$side: node created" FAIL "нет 'node created' в mush-логе"
    fi
    if grep -q "listener started" "$log"; then
        check "$side: listener started" PASS
    else
        check "$side: listener started" FAIL "нет 'listener started' (нода не поднялась?)"
    fi
    if grep -qF "$PUBKEY_PATTERN" "$log"; then
        pk=$(grep -F "$PUBKEY_PATTERN" "$log" | head -1)
        check "$side: pubkey в логе" PASS "$pk"
    else
        check "$side: pubkey в логе" MANUAL \
            "паттерн '$PUBKEY_PATTERN' не найден (переопредели HW_PUBKEY_PATTERN); возьми pubkey из UI/Native.pubkey"
    fi
done

# Session ID (§6.3): если паттерн найден на обеих сторонах — сравниваем,
# иначе MANUAL с инструкцией (в логах его может не быть — это нормально).
o_sid=$(grep -F "$SESSION_PATTERN" "$LOGS_DIR/$OWNER_TAG.log" 2>/dev/null | head -1 || true)
c_sid=$(grep -F "$SESSION_PATTERN" "$LOGS_DIR/$CLIENT_TAG.log" 2>/dev/null | head -1 || true)
if [ -n "$o_sid" ] && [ -n "$c_sid" ]; then
    if [ "$o_sid" = "$c_sid" ]; then
        check "session ID совпадает на обеих сторонах" PASS "$o_sid"
    else
        check "session ID совпадает на обеих сторонах" FAIL "owner='$o_sid' client='$c_sid'"
    fi
else
    check "session ID совпадает на обеих сторонах" MANUAL \
        "паттерн '$SESSION_PATTERN' не на обеих сторонах; сверь short-id в debug-поле Contacts (§6.3)"
fi

# ---------------------------------------------------------------------------
# 6. Ручная часть — guided.
# ---------------------------------------------------------------------------
say "== Ручная часть (оператор) =="
say "  1. OWNER ($OWNER): открой QR-invite (экран соединения)."
say "  2. CLIENT ($CLIENT): отсканируй QR, дождись handshake."
say "  3. CLIENT: отправь сообщение; OWNER: ответь."
say "  4. OWNER: проверь, что сообщение дошло; зафиксируй session ID (§6.3)."
read -r -p "  Нажми Enter когда пройдёшь шаги 1–4... " _

say "== Досбор логов после ручной части =="
for id in "$OWNER" "$CLIENT"; do
    tag="$id"; [ "$id" = "$OWNER" ] && [ "$id" = "$CLIENT" ] && tag="loopback"
    adb -s "$id" logcat -d -s mush:V AndroidRuntime:E >>"$LOGS_DIR/$tag.log" 2>&1 || true
done
# Крашей не должно быть ни на одном устройстве.
for tag in "$OWNER_TAG" "$CLIENT_TAG"; do
    if ! grep -q "AndroidRuntime" "$LOGS_DIR/$tag.log" 2>/dev/null; then
        check "$tag: без FATAL/AndroidRuntime" PASS
    else
        check "$tag: без FATAL/AndroidRuntime" FAIL "$(grep 'AndroidRuntime' "$LOGS_DIR/$tag.log" | head -1)"
    fi
done

# ---------------------------------------------------------------------------
# 7. Артефакт + сводка.
# ---------------------------------------------------------------------------
ARTIFACT="$SCRIPT_DIR/hardware-verify-$(date +%Y%m%d-%H%M%S).log"
{
    say "hardware-check artifact $(date -Is)"
    say "OWNER=$OWNER CLIENT=$CLIENT ABI/устройства: $(adb devices | tr '\n' ';')"
    say "APK=$APK wait=${WAIT_SEC}s pubkey_pattern='$PUBKEY_PATTERN' session_pattern='$SESSION_PATTERN'"
    say ""
    say "== Сводка =="
    printf '%s\n' "${RESULTS[@]}"
    say ""
    say "PASS=$PASS FAIL=$FAIL MANUAL=$MANUAL"
    say ""
    say "== Логи устройств (mush:V AndroidRuntime:E) =="
    for tag in "$OWNER_TAG" "$CLIENT_TAG"; do
        say "---- $tag ----"
        cat "$LOGS_DIR/$tag.log" 2>/dev/null || say "(нет)"
    done
} >"$ARTIFACT"
say ""
say "Артефакт: $ARTIFACT"
say "Сводка: PASS=$PASS FAIL=$FAIL MANUAL=$MANUAL"
if [ "$FAIL" -gt 0 ]; then
    say "ИТОГ: FAIL ($FAIL) — чеклист НЕ пройден"
    exit 1
fi
say "ИТОГ: нет FAIL — автоматическая часть пройдена (MANUAL=$MANUAL требуют оператора)"
exit 0
