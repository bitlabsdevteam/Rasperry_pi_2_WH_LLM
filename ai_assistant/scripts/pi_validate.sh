#!/usr/bin/env sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT_DIR"

BIN="./target/release/ai_assistant"

echo "== release build =="
"$ROOT_DIR/scripts/release.sh"

echo
echo "== command smoke =="
"$BIN" help
"$BIN" task list || true
"$BIN" serve --once || true

echo
echo "== cpu and memory samples =="
"$BIN" schedule add sample 0 "task add sample" >/dev/null 2>&1 || true
"$BIN" serve >/tmp/ai_assistant_serve.log 2>&1 &
PID=$!
sleep 3
for _ in 1 2 3; do
  date
  ps -o pid,%cpu,rss,command -p "$PID" 2>/dev/null || true
  sleep 2
done
kill "$PID" >/dev/null 2>&1 || true
wait "$PID" >/dev/null 2>&1 || true
echo "--- serve log ---"
sed -n '1,40p' /tmp/ai_assistant_serve.log 2>/dev/null || true

echo
echo "== thermal =="
if command -v vcgencmd >/dev/null 2>&1; then
  vcgencmd measure_temp
else
  echo "vcgencmd not available"
fi

echo
echo "== reboot persistence checklist =="
echo "1. Install deploy/ai_assistant.service into systemd."
echo "2. Enable and start the service."
echo "3. Reboot the Pi."
echo "4. Confirm the service is active and data/assistant.db persisted."
