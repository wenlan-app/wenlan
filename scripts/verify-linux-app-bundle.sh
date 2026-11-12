#!/usr/bin/env bash
# Proves a Linux desktop build is something a person can install and open,
# rather than trusting that `tauri build` exiting 0 means that.
#
#   bash scripts/verify-linux-app-bundle.sh <bundle-dir>
#
# <bundle-dir> is Tauri's output, e.g. target/x86_64-unknown-linux-gnu/release/bundle.
# It checks, in order:
#   1. exactly one .AppImage and one .deb were built, each with a non-empty
#      updater signature beside it;
#   2. both packages carry the app and all three sidecars (wenlan-server,
#      wenlan-mcp, wenlan) plus a desktop entry, so the app can start its
#      daemon and the AI-tool setup can find the CLI and MCP server;
#   3. the AppImage, unpacked, opens under a virtual display and the daemon it
#      starts from inside the bundle answers /api/health. That is the one step that
#      shows sidecar lookup works from a real Linux package. It runs against a
#      scratch HOME, data dir and port, and stops both processes afterwards.
#
# Set WENLAN_LINUX_SMOKE=0 to skip step 3 (only for a machine with no Xvfb).
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <bundle-dir>" >&2
  exit 2
fi
bundle_dir="$1"
[[ -d "$bundle_dir" ]] || { echo "error: no bundle directory at $bundle_dir" >&2; exit 1; }

main_bin="wenlan-app"
sidecars=(wenlan-server wenlan-mcp wenlan)

find_one() {
  local dir="$1" pattern="$2" found
  mapfile -t found < <(find "$dir" -maxdepth 1 -type f -name "$pattern" | sort)
  if [[ "${#found[@]}" -ne 1 ]]; then
    echo "error: expected exactly one $pattern under $dir, found ${#found[@]}" >&2
    return 1
  fi
  printf '%s\n' "${found[0]}"
}

appimage="$(find_one "$bundle_dir/appimage" '*.AppImage')"
deb="$(find_one "$bundle_dir/deb" '*.deb')"
for signed in "$appimage" "$deb"; do
  if [[ ! -s "$signed.sig" ]]; then
    echo "error: $signed.sig is missing or empty; the updater cannot trust this package" >&2
    exit 1
  fi
done
echo "PASS packages: $(basename "$appimage"), $(basename "$deb") (both signed)"

work="$(mktemp -d)"
app_pid=""
xvfb_pid=""
# The app renames its own process ("wenlan-app"), so its command line does
# not show where it runs from. Its executable path does: every process whose
# binary lives in this scratch tree is one this script started.
scratch_pids() {
  local exe pid
  for exe in /proc/[0-9]*/exe; do
    pid="${exe#/proc/}"
    pid="${pid%/exe}"
    if [[ "$(readlink "$exe" 2>/dev/null)" == "$work/"* ]]; then
      printf '%s\n' "$pid"
    fi
  done
}
cleanup() {
  local pids
  if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
    kill "$app_pid" 2>/dev/null || true
  fi
  pids="$(scratch_pids)"
  if [[ -n "$pids" ]]; then
    # shellcheck disable=SC2086
    kill $pids 2>/dev/null || true
    sleep 3
    pids="$(scratch_pids)"
    # shellcheck disable=SC2086
    [[ -z "$pids" ]] || kill -9 $pids 2>/dev/null || true
  fi
  if [[ -n "$xvfb_pid" ]]; then
    kill "$xvfb_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# --- deb payload -------------------------------------------------------------
deb_listing="$(dpkg-deb -c "$deb")"
failed=false
for name in "$main_bin" "${sidecars[@]}"; do
  if grep -Eq "[[:space:]](\./)?usr/bin/${name}\$" <<<"$deb_listing"; then
    echo "PASS deb ships usr/bin/$name"
  else
    echo "FAIL deb does not ship usr/bin/$name"
    failed=true
  fi
done
if grep -Eq '[[:space:]](\./)?usr/share/applications/[^/]+\.desktop$' <<<"$deb_listing"; then
  echo "PASS deb ships a desktop entry"
else
  echo "FAIL deb has no desktop entry"
  failed=true
fi

# --- AppImage payload --------------------------------------------------------
# Unpacking needs no FUSE, which a CI runner (and many containers) lacks.
cp "$appimage" "$work/Wenlan.AppImage"
chmod +x "$work/Wenlan.AppImage"
(cd "$work" && ./Wenlan.AppImage --appimage-extract >/dev/null)
appdir="$work/squashfs-root"
for name in "$main_bin" "${sidecars[@]}"; do
  if [[ -x "$appdir/usr/bin/$name" ]]; then
    echo "PASS AppImage ships usr/bin/$name"
  else
    echo "FAIL AppImage does not ship usr/bin/$name"
    failed=true
  fi
done
if [[ "$failed" == "true" ]]; then
  echo "error: the Linux packages are missing files the app needs" >&2
  exit 1
fi

if [[ "${WENLAN_LINUX_SMOKE:-1}" == "0" ]]; then
  echo "SKIP launch smoke (WENLAN_LINUX_SMOKE=0)"
  exit 0
fi

# --- launch smoke ------------------------------------------------------------
command -v Xvfb >/dev/null || { echo "error: Xvfb is required for the launch smoke" >&2; exit 1; }
port="${WENLAN_LINUX_SMOKE_PORT:-17931}"
if curl -fsS --max-time 2 "http://127.0.0.1:$port/api/health" >/dev/null 2>&1; then
  echo "error: something already answers on port $port; pick another with WENLAN_LINUX_SMOKE_PORT" >&2
  exit 1
fi
home="$work/home"
mkdir -p "$home"
log="$work/app.log"
# A display of our own, so stopping it afterwards cannot touch anyone else's.
display_num="${WENLAN_LINUX_SMOKE_DISPLAY:-117}"
if [[ -e "/tmp/.X11-unix/X$display_num" || -e "/tmp/.X$display_num-lock" ]]; then
  echo "error: display :$display_num is taken; pick another with WENLAN_LINUX_SMOKE_DISPLAY" >&2
  exit 1
fi
Xvfb ":$display_num" -screen 0 1440x900x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
xvfb_pid=$!
for _ in $(seq 1 50); do
  [[ -e "/tmp/.X11-unix/X$display_num" ]] && break
  sleep 0.2
done
if [[ ! -e "/tmp/.X11-unix/X$display_num" ]]; then
  echo "error: Xvfb did not start on :$display_num" >&2
  cat "$work/xvfb.log" >&2
  exit 1
fi
# AppRun sets up the bundled library paths the same way a double-click does.
HOME="$home" \
  XDG_CONFIG_HOME="$home/.config" \
  XDG_DATA_HOME="$home/.local/share" \
  XDG_CACHE_HOME="$home/.cache" \
  WENLAN_DATA_DIR="$home/wenlan-data" \
  WENLAN_PORT="$port" \
  WENLAN_NO_AUTOSTART=1 \
  NO_AT_BRIDGE=1 \
  DISPLAY=":$display_num" \
  "$appdir/AppRun" >"$log" 2>&1 &
app_pid=$!

deadline=$((SECONDS + ${WENLAN_LINUX_SMOKE_TIMEOUT:-180}))
healthy=false
while [[ "$SECONDS" -lt "$deadline" ]]; do
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "FAIL the app exited before its daemon came up. Its output:" >&2
    cat "$log" >&2
    exit 1
  fi
  if curl -fsS --max-time 2 "http://127.0.0.1:$port/api/health" >/dev/null 2>&1; then
    healthy=true
    break
  fi
  sleep 2
done
if [[ "$healthy" != "true" ]]; then
  echo "FAIL the daemon the app starts never answered /api/health on port $port. App output:" >&2
  cat "$log" >&2
  exit 1
fi
server_pids=""
for pid in $(scratch_pids); do
  if [[ "$(readlink "/proc/$pid/exe" 2>/dev/null)" == "$appdir/usr/bin/wenlan-server" ]]; then
    server_pids="$server_pids $pid"
  fi
done
server_pids="${server_pids# }"
if [[ -z "$server_pids" ]]; then
  echo "FAIL /api/health answered, but not from the wenlan-server inside this bundle" >&2
  exit 1
fi
echo "PASS the app opened and started the bundled daemon (pid $server_pids, port $port)"

# Still running a few seconds later: a webview that dies on first paint would
# have passed the health check above.
sleep 5
if ! kill -0 "$app_pid" 2>/dev/null; then
  echo "FAIL the app exited after its daemon started. Its output:" >&2
  cat "$log" >&2
  exit 1
fi
echo "PASS the app is still running after the daemon came up"
