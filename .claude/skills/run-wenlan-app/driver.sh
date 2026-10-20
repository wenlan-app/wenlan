#!/usr/bin/env bash
# Agent driver for the wenlan-app Tauri dev build. See SKILL.md next to this file.
# Subcommands: build | vite | launch | shot [out.png] | stop
#
# Every check here fails closed: a port, process or binary this cannot prove
# belongs to THIS checkout is refused, never reused and never killed. Another
# session's vite and app share the machine (once, a launch from one worktree
# silently loaded another checkout's frontend from a shared :1420).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)" # unit root (repo or worktree)
ROOT_REAL="$(cd "$ROOT" && pwd -P)"           # what lsof reports as a cwd
MAIN="$(cd "$(git -C "$ROOT" rev-parse --git-common-dir)/.." && pwd)"
# Cargo builds in the main checkout's shared, warm target (a worktree-local one
# costs ~14 GB). That target's debug/wenlan-app is whichever checkout built
# last, so it is never launched: `build` clones it, with its sidecars, into
# this checkout's own dir and proves the clone carries this checkout's values.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$MAIN/target}"
BUILT="$CARGO_TARGET_DIR/debug"
APP_DIR="${WENLAN_DEV_APP_DIR:-$ROOT/target/wenlan-dev-app}" # override: driver.test.sh only
BIN="$APP_DIR/wenlan-app"
STAMP="$APP_DIR/stamp"
APP_PID_FILE="$APP_DIR/app.pid"
# `.sidecar("wenlan-server")` resolves next to the executable, so the sidecars
# tauri-build copied beside the binary travel with it.
SIDECARS=(wenlan wenlan-server wenlan-mcp)
LOG="${TMPDIR:-/tmp}"

# Tri-state port, liveness and image probes; see scripts/AGENTS.md.
# shellcheck source=../../../scripts/lib/host-process.sh
source "$ROOT/scripts/lib/host-process.sh"

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

# Same override scripts/dev-all.sh hands the tauri CLI. tauri-build declares
# `rerun-if-env-changed=TAURI_CONFIG` and tauri-codegen json_patch-merges it,
# so a different value rebuilds the app crate (only that crate).
tauri_config() {
  printf '{"identifier":"%s","build":{"devUrl":"%s"}}' "$WENLAN_DEV_APP_ID" "$UI_URL"
}

stamp_text() {
  printf 'root=%s\ndevUrl=%s\nidentifier=%s\n' "$ROOT_REAL" "$UI_URL" "$WENLAN_DEV_APP_ID"
}

# stdout: the process's cwd. exit: 0 measured, 2 could not measure. A live
# process always has a cwd, so there is no negative: anything but exactly one
# path line is a failed read, never "some other directory".
process_cwd() {
  local out rc=0
  out="$(lsof -w -a -p "$1" -d cwd -Fn 2>&1)" || rc=$?
  ((rc == 0)) || return 2
  out="$(printf '%s\n' "$out" | sed -n 's/^n//p')"
  [[ -n "$out" && "$out" != *$'\n'* ]] || return 2
  printf '%s' "$out"
}

# Sets PORT_OWNER to free | mine | foreign | unmeasured, and PORT_PID.
probe_port_owner() {
  local cwd="" rc=0
  PORT_PID=""
  probe_listener_port "$1"
  case "$LISTENER_PROBE_STATE" in
    none) PORT_OWNER=free; return ;;
    unmeasured) PORT_OWNER=unmeasured; return ;;
  esac
  PORT_PID="$LISTENER_PROBE_PID"
  cwd="$(process_cwd "$PORT_PID")" || rc=$?
  if ((rc != 0)); then
    PORT_OWNER=unmeasured
  elif [[ "$cwd" == "$ROOT_REAL" ]]; then
    PORT_OWNER=mine
  else
    PORT_OWNER=foreign
    PORT_CWD="$cwd"
  fi
}

refuse_port() {
  case "$PORT_OWNER" in
    foreign)
      echo "REFUSING: :$1 is served by pid $PORT_PID from $PORT_CWD, not this" >&2
      echo "checkout ($ROOT_REAL). Reusing it would run that checkout's code." >&2
      echo "Never kill it; it belongs to another session." >&2
      ;;
    *) echo "REFUSING: could not measure who holds :$1 (sandboxed shell?)." >&2 ;;
  esac
  exit 1
}

# Sets APP_STATE to running | stopped | unmeasured, and APP_PID. Only the pid
# this checkout recorded, and only while it still runs this checkout's $BIN:
# never a process search, which matches every checkout's dev app.
probe_app() {
  APP_PID=""
  if [[ ! -f "$APP_PID_FILE" ]]; then
    APP_STATE=stopped
    return
  fi
  APP_PID="$(cat "$APP_PID_FILE")"
  probe_process_image "$APP_PID"
  case "$PROCESS_IMAGE_STATE" in
    found) [[ "$PROCESS_IMAGE_VALUE" == "$BIN" ]] && APP_STATE=running || APP_STATE=stopped ;;
    none) APP_STATE=stopped ;;
    *) APP_STATE=unmeasured ;;
  esac
}

clone_into_app_dir() {
  local name
  for name in wenlan-app "${SIDECARS[@]}"; do
    [[ -x "$BUILT/$name" ]] || {
      echo "BUILD WRONG: $BUILT/$name is missing" >&2
      return 1
    }
    # APFS clone: no extra disk until one side changes. Renamed into place so
    # a running app keeps its own inode.
    cp -c "$BUILT/$name" "$APP_DIR/$name.tmp" 2>/dev/null || cp "$BUILT/$name" "$APP_DIR/$name.tmp"
    mv -f "$APP_DIR/$name.tmp" "$APP_DIR/$name"
  done
}

case "${1:-}" in
  build)
    # Decoupled from `tauri dev`: its beforeDevCommand compiles sidecars and
    # times out the CLI's 180s wait for vite on a cold cache.
    load_dev_runtime_config
    cd "$ROOT"
    # The target is shared, and Cargo cannot tell checkouts apart in it: a
    # workspace crate's artifact names hash its path RELATIVE to the workspace
    # root, and freshness is mtime, so a checkout whose files are older than
    # another checkout's last build gets that build back, silently. Touch this
    # workspace's own package files so they are newer than any build in the
    # target. Not `cargo clean -p`: that deletes every checkout's artifacts for
    # these packages (70 GiB once), test binaries and incremental caches too.
    # Force the sidecar step, which otherwise reuses any sidecar binary already
    # in the target without running cargo.
    pkg_dirs="$(cargo metadata --no-deps --format-version 1 --manifest-path app/Cargo.toml |
      python3 -c 'import json,os,sys; [print(os.path.dirname(p["manifest_path"])) for p in json.load(sys.stdin)["packages"]]')" || {
      echo "BUILD WRONG: cargo metadata failed" >&2
      exit 1
    }
    dirs=()
    while read -r d; do [[ -n "$d" ]] && dirs+=("$d"); done <<<"$pkg_dirs"
    ((${#dirs[@]} > 0)) || {
      echo "BUILD WRONG: cargo metadata listed no workspace packages" >&2
      exit 1
    }
    git ls-files -z -co --exclude-standard -- "${dirs[@]}" Cargo.toml Cargo.lock | xargs -0 touch -c # -c: skip dangling fixture symlinks
    bash scripts/prepare-sidecars.sh --force-build
    TAURI_CONFIG="$(tauri_config)" cargo build --manifest-path app/Cargo.toml
    mkdir -p "$APP_DIR"
    rm -f "$STAMP"
    clone_into_app_dir
    # Prove the clone carries this checkout's values instead of trusting cargo's
    # rebuild tracking, or that no other checkout built in between.
    for want in "$UI_URL" "$WENLAN_DEV_APP_ID"; do
      if ! grep -aqF "$want" "$BIN"; then
        echo "BUILD WRONG: $BIN does not contain $want" >&2
        exit 1
      fi
    done
    stamp_text >"$STAMP"
    echo "built $BIN (devUrl $UI_URL)"
    ;;
  vite)
    load_dev_runtime_config
    cd "$ROOT"
    probe_port_owner "$WENLAN_DEV_UI_PORT"
    case "$PORT_OWNER" in
      free) nohup pnpm dev >"$LOG/wenlan-vite.log" 2>&1 & ;;
      mine) ;;
      *) refuse_port "$WENLAN_DEV_UI_PORT" ;;
    esac
    for _ in $(seq 1 30); do
      curl -s -o /dev/null "$UI_URL/" && break
      sleep 2
    done
    # Re-check: something else could have bound the port meanwhile.
    probe_port_owner "$WENLAN_DEV_UI_PORT"
    [[ "$PORT_OWNER" == mine ]] || refuse_port "$WENLAN_DEV_UI_PORT"
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
    probe_app
    case "$APP_STATE" in
      running)
        echo "APP ALREADY UP (pid $APP_PID); run stop first to relaunch"
        exit 0
        ;;
      unmeasured)
        echo "REFUSING: could not tell whether pid $APP_PID is this checkout's app." >&2
        exit 1
        ;;
    esac
    # The app now selects the worktree daemon port, so give it a daemon there.
    # This never touches the user's :7878 instance.
    bash "$ROOT/scripts/dev-runtime.sh" start >"$LOG/wenlan-dev-daemon.log" 2>&1 ||
      echo "warning: isolated dev daemon did not start; see $LOG/wenlan-dev-daemon.log" >&2
    "$0" vite
    # Rebuild unless this checkout's clone was built for this UI port.
    if [[ ! -x "$BIN" || "$(cat "$STAMP" 2>/dev/null)" != "$(stamp_text)" ]]; then
      "$0" build
    fi
    # Launch the binary directly: spawning through the pnpm/tauri chain in a
    # sandboxed shell panics on the rolling-log appender (PermissionDenied,
    # ~/Library/Logs/com.wenlan.desktop/).
    cd "$ROOT"
    nohup "$BIN" >"$LOG/wenlan-app.log" 2>&1 &
    echo "$!" >"$APP_PID_FILE"
    sleep 5
    probe_app
    if [[ "$APP_STATE" == running ]]; then
      echo "APP UP (pid $APP_PID, devUrl $UI_URL)"
    else
      echo "APP DEAD ($APP_STATE)"
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
    failed=0
    probe_app
    case "$APP_STATE" in
      running)
        kill "$APP_PID" 2>/dev/null || true
        for _ in 1 2 3 4 5; do
          probe_app
          [[ "$APP_STATE" == running ]] || break
          sleep 1
        done
        if [[ "$APP_STATE" == stopped ]]; then rm -f "$APP_PID_FILE"; else
          echo "warning: dev app pid $APP_PID NOT stopped ($APP_STATE)" >&2
          failed=1
        fi
        ;;
      stopped) rm -f "$APP_PID_FILE" ;;
      *)
        echo "warning: dev app NOT stopped: could not measure pid $APP_PID (sandboxed shell?)" >&2
        failed=1
        ;;
    esac
    # 1420 covers a vite this checkout started under the old pinned port.
    for port in "$WENLAN_DEV_UI_PORT" 1420; do
      probe_port_owner "$port"
      case "$PORT_OWNER" in
        mine) kill "$PORT_PID" 2>/dev/null || true ;;
        free) ;;
        foreign) echo "left :$port alone: it belongs to $PORT_CWD" >&2 ;;
        *)
          echo "warning: could not measure :$port; vite NOT stopped" >&2
          failed=1
          ;;
      esac
    done
    bash "$ROOT/scripts/dev-runtime.sh" stop || true
    exit "$failed"
    ;;
  *)
    echo "usage: driver.sh build|vite|launch|shot [out.png]|stop"
    exit 2
    ;;
esac
