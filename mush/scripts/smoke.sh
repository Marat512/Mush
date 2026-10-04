#!/usr/bin/env bash
# Smoke test: two real mush-node processes on one machine.
set -u
cd "$(dirname "$0")/.."
BIN=./target/release/mush-node
S=/tmp/mush-smoke
pkill -f 'mush-node --name' 2>/dev/null
sleep 0.5
rm -rf "$S" && mkdir -p "$S"

# Alice lives for the whole scenario (15s).
( { sleep 15; echo "/quit"; } | "$BIN" --name Alice --port 7878 > "$S/alice.log" 2>&1 ) &
ALICE_PID=$!
sleep 1.5

grep 'Invite link:' "$S/alice.log" | grep -o 'mush://[^ ]*' > "$S/invite.txt"
INVITE=$(cat "$S/invite.txt")
echo "invite: ${INVITE:0:80}..."

# Bob: join -> send -> decoy -> alert -> quit.
( {
    echo "/join $INVITE"
    sleep 1.5
    echo "/send default privet alice"
    sleep 0.7
    echo "/decoy"
    sleep 1.2
    echo "/alert"
    sleep 0.3
    echo "/quit"
  } | "$BIN" --name Bob --port 7879 > "$S/bob.log" 2>&1 ) &
BOB_PID=$!

wait "$BOB_PID" 2>/dev/null
sleep 1.5
kill "$ALICE_PID" 2>/dev/null
pkill -f 'mush-node --name' 2>/dev/null

echo "=== ALICE ==="
cat "$S/alice.log"
echo
echo "=== BOB ==="
cat "$S/bob.log"

# Assertions on the transcript.
A="$S/alice.log"; B="$S/bob.log"
ok=1
check() { if grep -q "$1" "$2"; then echo "PASS: $3"; else echo "FAIL: $3"; ok=0; fi }
check 'Handshake complete with Bob' "$A" "handshake Alice<-Bob"
check 'privet alice' "$A" "message delivered to Alice"
check 'Session key established' "$B" "session key on Bob"
check 'Sent bundle to decoy EID' "$B" "decoy bundle sent"
check 'S7 Alert' "$A" "S7 alert fired on Alice"
check 'Group joined' "$B" "Bob joined group"
[ "$ok" = 1 ] && echo "SMOKE: ALL PASS" || echo "SMOKE: FAILURES"
exit $((1-ok))
