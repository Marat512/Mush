#!/usr/bin/env bash
# inbound-probe.sh — обязательная inbound-проба устройства до старта пайплайна
# (MUSH-HARDWARE-TEST-PLAN §4.1): поднимает listener Mush на owner'е, TCP-connect
# к нему с client'а (НЕ через adb forward — он loopback и вендорный блок не
# показывает), проверяет `accepted connection` в logcat owner'а.
#
# Использование: scripts/inbound-probe.sh <owner_serial> <client_serial> [seconds]
# Требует: debug APK установлен на ОБОИХ устройствах, оба в одной L2-сети
# (роутер LAN), сервис поднят (am start -n app.mush/.MainActivity).
set -u

ADB="${ADB:-adb}"
OWNER="${1:-}"
CLIENT="${2:-}"
WAIT="${3:-45}"

die() { echo "FAIL: $*" >&2; exit 1; }
[ -n "$OWNER" ] && [ -n "$CLIENT" ] || die "usage: $0 <owner_serial> <client_serial> [seconds]"

# --- owner: listener + LAN-адвертайз (не hotspot) ---------------------------
"$ADB" -s "$OWNER" shell "am broadcast -a app.mush.debug.CMD --es cmd start" >/dev/null
sleep 8

LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT
"$ADB" -s "$OWNER" logcat -v threadtime mush:* *:S > "$LOG" 2>&1 &
CATPID=$!
sleep 1

# Auto-resume (день 8): после рестарта сервис мог сам поднять listener ДО
# старта стрима — строка 'listener started' уже в прошлом буфере (256 KiB
# ротируют за секунды). Поэтому порт берём из строки lan-адвертайза:
# 'lan mode: advertised <ip>:<port>' (п.18д §3.5, прогон 14.10).
"$ADB" -s "$OWNER" shell "am broadcast -a app.mush.debug.CMD --es cmd lan" >/dev/null
DEADLINE=$(( $(date +%s) + 30 ))
PORT=""
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    PORT="$(grep -oE 'advertised [0-9.]+:[0-9]+' "$LOG" | tail -1 | grep -oE '[0-9]+$')"
    [ -n "$PORT" ] && [ "$PORT" != "0" ] && break
    PORT=""
    sleep 2
done
[ -n "$PORT" ] || { kill "$CATPID" 2>/dev/null; die "listener на $OWNER не поднялся (нет 'lan mode: advertised <ip>:<port>' в logcat)"; }
LANIP="$("$ADB" -s "$OWNER" shell "ip -4 addr show wlan0 2>/dev/null | grep -oE '192\.168\.[0-9]+\.[0-9]+' | head -1" | tr -d '\r')"
[ -n "$LANIP" ] || die "у $OWNER нет IPv4 на wlan0 — не в LAN?"
echo "probe: owner=$OWNER listener=$LANIP:$PORT, connect с $CLIENT ..."

# --- client: TCP-connect к owner:port (raw, без Mush-протокола) -------------
"$ADB" -s "$CLIENT" shell "echo | nc -w 5 $LANIP $PORT" >/dev/null 2>&1

# --- ассерт: accepted connection у owner'а ----------------------------------
DEADLINE=$(( $(date +%s) + WAIT ))
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    if grep -q "accepted connection from" "$LOG"; then
        kill "$CATPID" 2>/dev/null
        echo "PASS: $(grep 'accepted connection from' "$LOG" | tail -1)"
        exit 0
    fi
    sleep 2
done
kill "$CATPID" 2>/dev/null
echo "FAIL: 'accepted connection from' не появился у owner'а $OWNER за ${WAIT}s." >&2
echo "Это вендорный блок inbound (§3.6): whitelist-тогглы вендора или смена owner'а." >&2
exit 1
