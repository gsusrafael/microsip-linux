#!/usr/bin/env bash
# Pair-test the sip-lab PBX with two real pjsua (PJSIP) softphones.
#
# pjsua is the same library MicroSIP is built on, so this exercises the PBX
# against a real-world SIP stack instead of only our scripted UA.
#
# - callee: --auto-answer 200, null audio (never touches the sound card)
# - caller: dials immediately (URL argument), --duration auto-hangs up
# - both register through the PBX with digest auth
#
# Usage: ./run_pjsua_test.sh          (PBX must be up on 127.0.0.1:5060)
set -uo pipefail

LAB="$(cd "$(dirname "$0")" && pwd)"
PJSUA="${PJSUA:-/home/jesus/.local/src/pjsip/pjproject/pjsip-apps/bin/pjsua-x86_64-unknown-linux-gnu}"
LOGDIR="$LAB/logs"
mkdir -p "$LOGDIR"

[ -x "$PJSUA" ] || { echo "FATAL: pjsua not found at $PJSUA"; exit 2; }

if ! ss -uln 2>/dev/null | grep -q '127.0.0.1:5060'; then
  echo "FATAL: PBX not listening on 127.0.0.1:5060 — start it first: python3 pbx.py"
  exit 2
fi

read -r PASS1 PASS2 <<<"$(python3 - "$LAB/accounts.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(d["users"]["1001"]["password"], d["users"]["1002"]["password"])
PY
)"

CALLEE_LOG="$LOGDIR/pjsua-callee.log"
CALLER_LOG="$LOGDIR/pjsua-caller.log"
rm -f "$CALLEE_LOG" "$CALLER_LOG"
# pjsua keeps its own console file next to the log unless told otherwise
rm -f "$LAB/pjsua.log" "$LAB/pjsua.cfg"

# Keep a stdin open for pjsua's console (it must not see EOF). A sleep pipe
# gives it an idle stdin that never delivers data and never closes.
IDLE_STDIN="$LOGDIR/.idle-stdin"
rm -f "$IDLE_STDIN"
mkfifo "$IDLE_STDIN"
sleep 120 > "$IDLE_STDIN" &
SLEEPER_PID=$!

echo "== starting callee pjsua (1002, auto-answer, port 5062, null audio)"
"$PJSUA" \
  --null-audio --no-color --log-level=4 --app-log-level=4 \
  --local-port=5062 --no-tcp --rtp-port=4010 \
  --id=sip:1002@127.0.0.1 --registrar=sip:127.0.0.1:5060 \
  --realm=sip-lab --username=1002 --password="$PASS2" \
  --auto-answer=200 \
  --log-file="$CALLEE_LOG" --log-append \
  < "$IDLE_STDIN" > "$LOGDIR/pjsua-callee.out" 2>&1 &
CALLEE_PID=$!
sleep 2

echo "== starting caller pjsua (1001, dials 1002, 8s call, port 5061, null audio)"
"$PJSUA" \
  --null-audio --no-color --log-level=4 --app-log-level=4 \
  --local-port=5061 --no-tcp --rtp-port=4000 \
  --id=sip:1001@127.0.0.1 --registrar=sip:127.0.0.1:5060 \
  --realm=sip-lab --username=1001 --password="$PASS1" \
  --duration=8 \
  --log-file="$CALLER_LOG" --log-append \
  'sip:1002@127.0.0.1:5060' \
  < "$IDLE_STDIN" > "$LOGDIR/pjsua-caller.out" 2>&1 &
CALLER_PID=$!

echo "== waiting for the call to complete (max 40s)"
for i in $(seq 1 40); do
  sleep 1
  if ! kill -0 "$CALLER_PID" 2>/dev/null; then
    echo "== caller exited after ${i}s"
    break
  fi
done

# stop leftovers by exact pid (never pkill)
for pid in "$CALLER_PID" "$CALLEE_PID" "$SLEEPER_PID"; do
  kill "$pid" 2>/dev/null || true
done
sleep 1
for pid in $(ss -ulnp 2>/dev/null | grep -E '127.0.0.1:506[12]' \
             | grep -oP 'pid=\K[0-9]+' | sort -u); do
  kill "$pid" 2>/dev/null || true
done
rm -f "$IDLE_STDIN"
sleep 1

echo
echo "==== pjsua evidence ===="
echo "-- caller (1001) key lines:"
grep -E 'egistration success|CONFIRMED|DISCONNECTED|Call .* state changed' \
  "$CALLER_LOG" 2>/dev/null | tail -20
echo "-- callee (1002) key lines:"
grep -E 'egistration success|CONFIRMED|DISCONNECTED|Incoming call|Call .* state changed' \
  "$CALLEE_LOG" 2>/dev/null | tail -20

count() { grep -ci "$1" "$2" 2>/dev/null | head -1; }
num() { v="${1:-0}"; v="${v//[^0-9]/}"; echo "${v:-0}"; }
CALLER_OK="$(num "$(count CONFIRMED "$CALLER_LOG")")"
CALLEE_OK="$(num "$(count CONFIRMED "$CALLEE_LOG")")"
CALLER_REG="$(num "$(count 'registration success' "$CALLER_LOG")")"
CALLEE_REG="$(num "$(count 'registration success' "$CALLEE_LOG")")"

echo
if [ "${CALLER_OK:-0}" -ge 1 ] && [ "${CALLEE_OK:-0}" -ge 1 ] \
   && [ "${CALLER_REG:-0}" -ge 1 ] && [ "${CALLEE_REG:-0}" -ge 1 ]; then
  echo "RESULT: PASS — both pjsua registered and the call reached CONFIRMED"
  echo "        caller(reg=$CALLER_REG confirmed=$CALLER_OK) callee(reg=$CALLEE_REG confirmed=$CALLEE_OK)"
  exit 0
fi
echo "RESULT: FAIL — caller(reg=$CALLER_REG confirmed=$CALLER_OK) callee(reg=$CALLEE_REG confirmed=$CALLEE_OK)"
echo "        see $CALLER_LOG and $CALLEE_LOG"
exit 1
