#!/usr/bin/env bash
# Ensure Vite is serving on :5173.
# Starts it in a new session so VS Code task teardown cannot kill it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UI="$ROOT/ui"
LOG=/tmp/manuscript-intel-vite.log
PIDFILE=/tmp/manuscript-intel-vite.pid

vite_up() {
  curl -sf --max-time 1 "http://127.0.0.1:5173/" >/dev/null 2>&1
}

if vite_up; then
  echo "VITE already ready in 0ms"
  exit 0
fi

# Clear a stale pidfile if the process is gone
if [[ -f "$PIDFILE" ]]; then
  old="$(cat "$PIDFILE" 2>/dev/null || true)"
  if [[ -n "${old:-}" ]] && ! kill -0 "$old" 2>/dev/null; then
    rm -f "$PIDFILE"
  fi
fi

echo "Starting Vite…"
# New session + detached: survives the VS Code task shell exiting
python3 - "$UI" "$LOG" "$PIDFILE" <<'PY'
import os, subprocess, sys
ui, log, pidfile = sys.argv[1], sys.argv[2], sys.argv[3]
os.chdir(ui)
with open(log, "ab", buffering=0) as out:
    proc = subprocess.Popen(
        ["npm", "run", "dev"],
        stdin=subprocess.DEVNULL,
        stdout=out,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        env=os.environ.copy(),
    )
with open(pidfile, "w") as f:
    f.write(str(proc.pid))
print(proc.pid)
PY

for _ in $(seq 1 60); do
  if vite_up; then
    echo "VITE ready in ${SECONDS}s"
    exit 0
  fi
  sleep 0.5
done

echo "Timed out waiting for Vite on :5173. Log:"
tail -n 40 "$LOG" || true
exit 1
