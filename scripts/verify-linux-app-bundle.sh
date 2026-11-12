#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Proves a Linux desktop build is something a person can install and open,
# rather than trusting that `tauri build` exiting 0 means that.
#
#   bash scripts/verify-linux-app-bundle.sh <bundle-dir>
#
# <bundle-dir> is Tauri's output, e.g. target/x86_64-unknown-linux-gnu/release/bundle.
# It checks, in order:
#   1. exactly one .AppImage and one .deb were built, and each one's updater
#      signature verifies (minisign) against the updater public key over the
#      exact package bytes. A non-empty .sig made with the wrong key would
#      otherwise ship, and every installed app would refuse the update;
#   2. both packages carry the app and all three sidecars (wenlan-server,
#      wenlan-mcp, wenlan) plus a desktop entry, and each package's app binary
#      carries its own Tauri bundle marker (AppImage vs deb). The updater picks
#      which package to download and how to install it from that marker;
#   3. the AppImage itself (through its runtime, extract-and-run, so no FUSE is
#      needed) opens under a virtual display, and the daemon it starts answers
#      /api/health. It runs as a fresh install would: a scratch HOME and no
#      data-dir override, so the step that copies the sidecars into
#      ~/.wenlan/bin and records the MCP path for the AI tools runs too, and is
#      checked. It stops every process it started afterwards.
#
# The public key is WENLAN_UPDATER_PUBKEY (base64, the form tauri.conf.json
# uses) or, unset, the one in app/tauri.conf.json.
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
command -v minisign >/dev/null || { echo "error: minisign is required to check the updater signatures" >&2; exit 1; }

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

# --- updater signatures --------------------------------------------------------
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pubkey_b64="${WENLAN_UPDATER_PUBKEY:-}"
if [[ -z "$pubkey_b64" ]]; then
  pubkey_b64="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["plugins"]["updater"]["pubkey"])' "$repo_root/app/tauri.conf.json")"
fi
printf '%s' "$pubkey_b64" | base64 -d >"$work/updater.pub" || { echo "error: the updater public key is not base64" >&2; exit 1; }
for signed in "$appimage" "$deb"; do
  if [[ ! -s "$signed.sig" ]]; then
    echo "error: $signed.sig is missing or empty; the updater cannot trust this package" >&2
    exit 1
  fi
  base64 -d <"$signed.sig" >"$work/package.minisig" || { echo "error: $signed.sig is not base64" >&2; exit 1; }
  if ! minisign -V -q -p "$work/updater.pub" -m "$signed" -x "$work/package.minisig"; then
    echo "error: $signed.sig does not verify against the updater public key; installed apps would refuse this update" >&2
    exit 1
  fi
done
echo "PASS packages: $(basename "$appimage"), $(basename "$deb") (signatures verify against the updater key)"

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
# Tauri patches a marker into each package's copy of the app; the updater reads
# it to choose linux-x86_64-deb or -appimage and how to install the download.
has_marker() {
  local file="$1" marker="$2" count
  count="$(grep -a -c -F "$marker" "$file" || true)"
  [[ "${count:-0}" -gt 0 ]]
}
mkdir -p "$work/deb-root"
dpkg-deb -x "$deb" "$work/deb-root"
if has_marker "$work/deb-root/usr/bin/$main_bin" __TAURI_BUNDLE_TYPE_VAR_DEB; then
  echo "PASS deb app is marked as a deb install"
else
  echo "FAIL deb app does not carry the deb bundle marker; the updater would fetch the wrong package"
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
if [[ -f "$appdir/usr/bin/$main_bin" ]] && has_marker "$appdir/usr/bin/$main_bin" __TAURI_BUNDLE_TYPE_VAR_APP; then
  echo "PASS AppImage app is marked as an AppImage install"
else
  echo "FAIL AppImage app does not carry the AppImage bundle marker; the updater would fetch the wrong package"
  failed=true
fi
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
# The AppImage runtime, not the unpacked AppRun: it sets APPIMAGE/APPDIR the
# way a double-click does, which is what turns on the ~/.wenlan/bin copies.
# Extract-and-run unpacks under TMPDIR, kept inside the scratch tree so the
# cleanup finds every process. No WENLAN_DATA_DIR: an override switches off
# the ~/.wenlan/bin step this checks; the scratch HOME holds the data instead.
mkdir -p "$work/tmp"
HOME="$home" \
  XDG_CONFIG_HOME="$home/.config" \
  XDG_DATA_HOME="$home/.local/share" \
  XDG_CACHE_HOME="$home/.cache" \
  XDG_STATE_HOME="$home/.local/state" \
  TMPDIR="$work/tmp" \
  APPIMAGE_EXTRACT_AND_RUN=1 \
  WENLAN_PORT="$port" \
  WENLAN_NO_AUTOSTART=1 \
  NO_AT_BRIDGE=1 \
  DISPLAY=":$display_num" \
  "$work/Wenlan.AppImage" >"$log" 2>&1 &
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
  exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  if [[ "$exe" == "$work/tmp/"*/usr/bin/wenlan-server || "$exe" == "$home/.wenlan/bin/wenlan-server" ]]; then
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

# The AppImage mount is gone once the app quits, so the CLI, the MCP server
# and the path the AI tools read must be real files in ~/.wenlan/bin.
bin_dir="$home/.wenlan/bin"
for name in "${sidecars[@]}"; do
  if [[ -f "$bin_dir/$name" && ! -L "$bin_dir/$name" && -x "$bin_dir/$name" ]] \
    && cmp -s "$bin_dir/$name" "$appdir/usr/bin/$name"; then
    echo "PASS ~/.wenlan/bin/$name is a lasting copy of the bundled binary"
  else
    echo "FAIL ~/.wenlan/bin/$name is not a lasting copy of the bundled binary:" >&2
    ls -la "$bin_dir" >&2 2>/dev/null || true
    cat "$log" >&2
    exit 1
  fi
done
recorded="$(cat "$bin_dir/wenlan-mcp.path" 2>/dev/null || true)"
if [[ "$recorded" == "$bin_dir/wenlan-mcp" ]]; then
  echo "PASS the AI tools are pointed at ~/.wenlan/bin/wenlan-mcp"
else
  echo "FAIL the recorded MCP path is '$recorded', expected $bin_dir/wenlan-mcp" >&2
  exit 1
fi
