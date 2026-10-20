#!/usr/bin/env bash
# Ownership contract for driver.sh: it never reuses, and never kills, a vite or
# dev app that another checkout started. No cargo, no GUI; stand-in processes
# play the other sessions. macOS, unsandboxed (the sandbox denies `ps`).
#
#   bash .claude/skills/run-wenlan-app/driver.test.sh
#
# Exit 0 all passed, 1 a contract broke, 2 could not run (UNCHECKED, not a pass).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DRIVER="${DRIVER_UNDER_TEST:-$HERE/driver.sh}" # override: negative controls
ROOT="$(cd "$HERE/../../.." && pwd)"
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/driver-test.XXXXXX")"
export WENLAN_DEV_APP_DIR="$SCRATCH/app"
mkdir -p "$WENLAN_DEV_APP_DIR" "$SCRATCH/elsewhere"
PIDS=()
failures=0

cleanup() {
  local pid
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

unchecked() {
  echo "UNCHECKED: $*" >&2
  exit 2
}
pass() { echo "ok   - $*"; }
fail() {
  echo "FAIL - $*" >&2
  failures=$((failures + 1))
}
alive() { kill -0 "$1" 2>/dev/null; }
# check <description> <command...>: pass when the command succeeds.
check() {
  local what="$1"
  shift
  if "$@"; then pass "$what"; else fail "$what"; fi
}
dead() { ! alive "$1"; }
no_record() { [[ ! -f "$WENLAN_DEV_APP_DIR/app.pid" ]]; }

ps -p $$ >/dev/null 2>&1 || unchecked "ps is denied (sandboxed shell?)"
command -v cc >/dev/null || unchecked "no C compiler for the stand-in app"
command -v python3 >/dev/null || unchecked "no python3 for the stand-in vite"

UI_PORT="$(bash "$ROOT/scripts/dev-runtime.sh" print-config 2>/dev/null | sed -n 's/^WENLAN_DEV_UI_PORT=//p')"
[[ -n "$UI_PORT" ]] || unchecked "dev-runtime.sh print-config gave no UI port"
if lsof -nP -iTCP:"$UI_PORT" -sTCP:LISTEN -t >/dev/null 2>&1; then
  unchecked ":$UI_PORT is in use; stop this checkout's vite first"
fi

# Stand-in app: an argv-free process, so its command line can equal $BIN.
printf '#include <unistd.h>\nint main(void){sleep(120);return 0;}\n' >"$SCRATCH/f.c"
cc -o "$WENLAN_DEV_APP_DIR/wenlan-app" "$SCRATCH/f.c" || unchecked "cc failed"
cp "$WENLAN_DEV_APP_DIR/wenlan-app" "$SCRATCH/elsewhere/wenlan-app"

# 1. Another checkout's vite holds this checkout's UI port.
(cd "$SCRATCH/elsewhere" && exec python3 -m http.server "$UI_PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
foreign_vite=$!
PIDS+=("$foreign_vite")
for _ in $(seq 1 20); do
  lsof -nP -iTCP:"$UI_PORT" -sTCP:LISTEN -t >/dev/null 2>&1 && break
  sleep 0.25
done
out="$("$DRIVER" vite 2>&1)"
rc=$?
if ((rc != 0)) && [[ "$out" == *"REFUSING: :$UI_PORT is served by pid"* ]]; then
  pass "vite refuses another checkout's listener on :$UI_PORT"
else
  fail "vite reused or misreported a foreign listener (rc=$rc): $out"
fi

# 2. The recorded pid now belongs to another checkout's app (pid reuse, or a
#    stale record). Same binary name, different path and cwd.
(cd "$SCRATCH/elsewhere" && exec "$SCRATCH/elsewhere/wenlan-app") &
foreign_app=$!
PIDS+=("$foreign_app")
# 3. This checkout's app, launched the way `launch` does: exact path, from ROOT.
(cd "$ROOT" && exec "$WENLAN_DEV_APP_DIR/wenlan-app") &
my_app=$!
PIDS+=("$my_app")
sleep 0.5

echo "$foreign_app" >"$WENLAN_DEV_APP_DIR/app.pid"
"$DRIVER" stop >/dev/null 2>&1
check "stop spares another checkout's app" alive "$foreign_app"
check "stop spares another checkout's vite" alive "$foreign_vite"

echo "$my_app" >"$WENLAN_DEV_APP_DIR/app.pid"
"$DRIVER" stop >/dev/null 2>&1
sleep 0.5
check "stop ends this checkout's app" dead "$my_app"
check "stop clears the record" no_record

# 4. No process-name search may come back: it matches every checkout's app.
if grep -E '\b(pgrep|pkill|killall)\b|lsof -ti' "$DRIVER" | grep -vE '^[[:space:]]*#'; then
  fail "driver.sh searches processes by name"
else
  pass "driver.sh has no name-based process search"
fi

if ((failures == 0)); then
  echo "driver.test.sh: all passed"
else
  echo "driver.test.sh: $failures failed" >&2
  exit 1
fi
