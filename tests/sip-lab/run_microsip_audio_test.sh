#!/usr/bin/env bash
# E2E del audio de MicroSIP contra el PBX local:
#   1001 (pjsua, null audio) -> 1002 (MicroSIP, auto-answer)
# Verifica REGISTER, timbre de llamada, CONFIRMED y DISCONNECTED.
set -euo pipefail

LAB="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$LAB/../.." && pwd)"
PJSUA="${PJSUA:-/home/jesus/.local/src/pjsip/pjproject/pjsip-apps/bin/pjsua-x86_64-unknown-linux-gnu}"
OUT="${OUT:-/tmp/microsip-audio-e2e-$$}"
mkdir -p "$OUT/microsip-linux"

[ -x "$PJSUA" ] || { echo "FATAL: pjsua not found at $PJSUA"; exit 2; }
if ss -uln 2>/dev/null | grep -q '127.0.0.1:5060'; then
  echo "FATAL: puerto 5060 ocupado; detén el PBX anterior"
  exit 2
fi
if xdpyinfo -display :199 >/dev/null 2>&1; then
  echo "FATAL: display :199 ocupado; elige otro Xvfb en el script"
  exit 2
fi

cat >"$OUT/microsip-linux/accounts.conf" <<'EOF'
id=1002@127.0.0.1
username=1002
password=lab-1002-test
domain=127.0.0.1
display_name=Bob
proxy=

EOF
cat >"$OUT/microsip-linux/audio.conf" <<EOF
capture_device=
playback_device=
ringtone=$REPO/assets/ringtone.wav

EOF
chmod 600 "$OUT/microsip-linux/accounts.conf" "$OUT/microsip-linux/audio.conf"

cd "$REPO"
cargo build --no-default-features --features pjsip,test-hooks >/dev/null

python3 "$LAB/pbx.py" >"$OUT/pbx.log" 2>&1 &
pbx_pid=$!
app_pid=
caller_pid=
sleeper_pid=
xvfb_pid=
cleanup() {
  for pid in "$caller_pid" "$app_pid" "$sleeper_pid" "$xvfb_pid"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  [ -n "$pbx_pid" ] && kill -TERM "$pbx_pid" 2>/dev/null || true
  wait "$pbx_pid" 2>/dev/null || true
  rm -f "$OUT/caller.stdin"
  wait 2>/dev/null || true
}
trap cleanup EXIT
sleep 1

export XDG_CONFIG_HOME="$OUT"
export RUST_LOG=info
export MICROSIP_TEST_AUTO_ANSWER=1
export NO_AT_BRIDGE=1
# Ejecutar el binario directamente dentro de un Xvfb propio permite matar
# el PID real de la app; matar solo el wrapper de xvfb-run dejaba huerfano
# el proceso GTK en algunas corridas.
Xvfb :199 -screen 0 1100x760x24 -nolisten tcp -ac >"$OUT/xvfb.log" 2>&1 &
xvfb_pid=$!
export DISPLAY=:199
sleep 1
./target/debug/microsip-linux >"$OUT/app.log" 2>"$OUT/app.err" &
app_pid=$!
sleep 3

mkfifo "$OUT/caller.stdin"
sleep 30 >"$OUT/caller.stdin" &
sleeper_pid=$!
"$PJSUA" --null-audio --no-color --log-level=4 --app-log-level=4 \
  --local-port=5061 --no-tcp --rtp-port=4010 \
  --id=sip:1001@127.0.0.1 --registrar=sip:127.0.0.1:5060 \
  --realm=sip-lab --username=1001 --password=lab-1001-test \
  --duration=5 --log-file="$OUT/caller.log" \
  'sip:1002@127.0.0.1:5060' \
  <"$OUT/caller.stdin" >"$OUT/caller.out" 2>&1 &
caller_pid=$!

for _ in $(seq 1 20); do
  if grep -q 'timbre detenido' "$OUT/app.log" \
    && grep -q 'CONFIRMED' "$OUT/caller.log" \
    && grep -q 'DISCONNECTED' "$OUT/caller.log"; then
    break
  fi
  sleep 1
done
kill "$caller_pid" 2>/dev/null || true
wait "$caller_pid" 2>/dev/null || true
sleep 1

grep -E 'registration success|timbre reproduciéndose|timbre detenido|CONFIRMED|DISCONNECTED' \
  "$OUT/app.log" "$OUT/caller.log" | tail -30

grep -q 'timbre reproduciéndose' "$OUT/app.log"
grep -q 'timbre detenido' "$OUT/app.log"
grep -q 'CONFIRMED' "$OUT/caller.log"
grep -q 'DISCONNECTED' "$OUT/caller.log"
echo "RESULT: PASS — registro, timbre, llamada confirmada y cierre"
echo "evidencia: $OUT"
