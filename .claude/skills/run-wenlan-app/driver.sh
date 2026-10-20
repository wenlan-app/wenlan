#!/usr/bin/env bash
# Agent driver for the wenlan-app Tauri dev build. See SKILL.md next to this file.
# Subcommands: build | vite | launch | shot [out.png] | stop
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)" # unit root (repo or worktree)
ROOT_REAL="$(cd "$ROOT" && pwd -P)"           # what lsof reports as a cwd
# Worktree-local by default. The app binary bakes in its devUrl and bundle
# identifier at build time, so a target dir shared across checkouts means one
# checkout's build is what another checkout launches.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$CARGO_TARGET_DIR/debug/wenlan-app"
STAMP="$BIN.driver-stamp"
LOG="${TMPDIR:-/tmp}"

# A debug wenlan-app refuses to start without a complete isolated runtime
# identity (see validate_debug_runtime_isolation in app/src/lib.rs), so the
# driver takes the same worktree-scoped config `pnpm dev:all` uses. This is
# also what keeps an agent launch off the user's daemon and LaunchAgents.
load_dev_runtime_config() {
  while IFS='=' read -r key value; do
    case "$key" in
      WENLAN_PORT | WENLAN_DEV_UI_PORT | WENLAN_DEV_REMOTE_PORT_START | WENLAN_DEV_APP_ID | WENLAN_DEV_TAURI_MCP_SOCKET | WENLAN_DATA_DIR | WENLAN_DEV_STATE_DIR)
        export "$key=$value"
        ;;
    esac
  done < <(bash "$ROOT/scripts/dev-runtime.sh" print-config)
  UI_URL="http://localhost:$WENLAN_DEV_UI_PORT"
}

# Same override scripts/dev-all.sh hands the tauri CLI: the worktree's UI port
# and dev bundle identifier, json_patch-merged by tauri-build and tauri-codegen.
tauri_config() {
  printf '{"identifier":"%s","build":{"devUrl":"%s"}}' "$WENLAN_DEV_APP_ID" "$UI_URL"
}

stamp_text() {
  printf 'root=%s\ndevUrl=%s\nidentifier=%s\n' "$ROOT_REAL" "$UI_URL" "$WENLAN_DEV_APP_ID"
}

pid_cwd() {
  lsof -a -p "$1" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p'
}

listener_pids() {
  lsof -nP -iTCP:"$1" -sTCP:LISTEN -t 2>/dev/null | sort -u
}

# Only a dev app this checkout launched: the exact $BIN command line (string
# compare, not a pattern) and a cwd of this unit root (launch cds there).
# The seatbelt sandbox denies ps and pgrep outright; fail rather than report
# "nothing running".
app_pids() {
  local pid cmd
  if ! ps -p $$ >/dev/null 2>&1; then
    echo "cannot list processes (sandboxed shell?); re-run unsandboxed" >&2
    return 1
  fi
  ps -axwwo pid=,command= | while read -r pid cmd; do
    [ "$cmd" = "$BIN" ] || continue
    [ "$(pid_cwd "$pid")" = "$ROOT_REAL" ] || continue
    echo "$pid"
  done
}

# Prints this checkout's vite PID on $1, nothing if the port is free, and exits
# non-zero if any other process holds it. Never reuse or kill a listener whose
# cwd is another checkout: that is another session's frontend.
own_listener() {
  local port="$1" pid cwd mine=""
  for pid in $(listener_pids "$port"); do
    cwd="$(pid_cwd "$pid")"
    if [ "$cwd" != "$ROOT_REAL" ]; then
      echo "REFUSING: :$port is served by pid $pid from ${cwd:-<unreadable cwd>}," >&2
      echo "not this checkout ($ROOT_REAL). Reusing it would run that checkout's" >&2
      echo "frontend. Never kill it; it belongs to another session." >&2
      return 1
    fi
    mine="$pid"
  done
  echo "$mine"
}

case "${1:-}" in
  build)
    # Decoupled from `tauri dev`: its beforeDevCommand compiles sidecars and
    # times out the CLI's 180s wait for vite on a cold cache.
    load_dev_runtime_config
    cd "$ROOT"
    pnpm prepare:sidecars
    TAURI_CONFIG="$(tauri_config)" cargo build --manifest-path app/Cargo.toml
    # Prove the override landed instead of trusting cargo's rebuild tracking.
    for want in "$UI_URL" "$WENLAN_DEV_APP_ID"; do
      if ! grep -aqF "$want" "$BIN"; then
        echo "BUILD WRONG: $BIN does not contain $want" >&2
        rm -f "$STAMP"
        exit 1
      fi
    done
    stamp_text >"$STAMP"
    echo "built $BIN (devUrl $UI_URL)"
    ;;
  vite)
    load_dev_runtime_config
    cd "$ROOT"
    pid="$(own_listener "$WENLAN_DEV_UI_PORT")" || exit 1
    if [ -z "$pid" ]; then
      nohup pnpm dev >"$LOG/wenlan-vite.log" 2>&1 &
    fi
    for _ in $(seq 1 30); do
      curl -s -o /dev/null "$UI_URL/" && break
      sleep 2
    done
    # Re-check ownership: something else could have bound the port meanwhile.
    own_listener "$WENLAN_DEV_UI_PORT" >/dev/null || exit 1
    curl -s -o /dev/null -w "vite $UI_URL: %{http_code}\n" "$UI_URL/"
    ;;
  launch)
    # The app inherits this shell's seatbelt sandbox, and the sandbox denies
    # the mach lookup for the ViewBridge XPC service that backs NSOpenPanel.
    # The app starts fine and dies hours later, on the first file/folder
    # picker: +[NSOpenPanel openPanel] returns NULL and objc2 panics, taking
    # the whole process with it — far enough from the launch to look like a
    # product bug. pbpaste needs the same class of mach lookup, so it is a
    # free canary for "this shell is sandboxed".
    if ! /usr/bin/pbpaste >/dev/null 2>&1; then
      echo "REFUSING TO LAUNCH: this shell is sandboxed." >&2
      echo "The app would inherit the sandbox and hard-crash on the first" >&2
      echo "file/folder picker (NSOpenPanel -> NULL -> panic). Re-run this" >&2
      echo "command with the sandbox disabled." >&2
      exit 1
    fi
    load_dev_runtime_config
    running="$(app_pids)" || exit 1
    if [ -n "$running" ]; then
      echo "APP ALREADY UP (pid $running); run stop first to relaunch"
      exit 0
    fi
    # The app now selects the worktree daemon port, so give it a daemon there.
    # This never touches the user's :7878 instance.
    bash "$ROOT/scripts/dev-runtime.sh" start >"$LOG/wenlan-dev-daemon.log" 2>&1 ||
      echo "warning: isolated dev daemon did not start; see $LOG/wenlan-dev-daemon.log" >&2
    "$0" vite
    # Rebuild unless the binary was built by this checkout for this UI port.
    if [ ! -x "$BIN" ] || [ "$(cat "$STAMP" 2>/dev/null)" != "$(stamp_text)" ]; then
      "$0" build
    fi
    # Launch the binary directly: spawning through the pnpm/tauri chain in a
    # sandboxed shell panics on the rolling-log appender (PermissionDenied,
    # ~/Library/Logs/com.wenlan.desktop/).
    cd "$ROOT"
    nohup "$BIN" >"$LOG/wenlan-app.log" 2>&1 &
    app_pid=$!
    sleep 5
    if app_pids | grep -qx "$app_pid"; then
      echo "APP UP (pid $app_pid, devUrl $UI_URL)"
    else
      echo "APP DEAD"
      tail -20 "$LOG/wenlan-app.log"
      exit 1
    fi
    ;;
  shot)
    # ScreenCaptureKit window capture — works while occluded; plain
    # `screencapture -l/-R` both fail ("could not create image from window").
    swift "$(dirname "$0")/wincap.swift" "${2:-$LOG/wenlan-shot.png}"
    ;;
  stop)
    # Only what this checkout started. NEVER kill the user's daemon on :7878
    # (dev-runtime.sh stop refuses any PID it did not record for this
    # worktree), nor another checkout's app or vite.
    load_dev_runtime_config
    if pids="$(app_pids)"; then
      for pid in $pids; do kill "$pid" 2>/dev/null || true; done
    else
      echo "warning: dev app NOT stopped" >&2
    fi
    # 1420 covers a vite this checkout started under the old pinned port.
    for port in "$WENLAN_DEV_UI_PORT" 1420; do
      pid="$(own_listener "$port" 2>/dev/null)" || continue
      [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    done
    bash "$ROOT/scripts/dev-runtime.sh" stop || true
    ;;
  *)
    echo "usage: driver.sh build|vite|launch|shot [out.png]|stop"
    exit 2
    ;;
esac
