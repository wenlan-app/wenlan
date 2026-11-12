#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Installs what `pnpm tauri build` needs to compile and bundle the Linux desktop
# app on an Ubuntu runner: the WebKitGTK 4.1 webview, the tray library, the
# SVG/icon loader and the tools the AppImage bundler shells out to. xvfb is
# for the launch smoke in scripts/verify-linux-app-bundle.sh.
#
# The daemon sidecars need nothing from here: they build on a stock runner, as
# the Linux CLI archives already do.
set -euo pipefail

packages=(
  libwebkit2gtk-4.1-dev
  libayatana-appindicator3-dev
  librsvg2-dev
  libssl-dev
  libxdo-dev
  patchelf
  file
  xdg-utils
  xvfb
)

sudo_cmd=()
if [[ "$(id -u)" -ne 0 ]]; then
  sudo_cmd=(sudo)
fi

attempt=1
until "${sudo_cmd[@]}" apt-get update -q; do
  if [[ "$attempt" -ge 3 ]]; then
    echo "error: apt-get update failed after $attempt attempts" >&2
    exit 1
  fi
  sleep $((15 * attempt))
  attempt=$((attempt + 1))
done

attempt=1
until "${sudo_cmd[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends "${packages[@]}"; do
  if [[ "$attempt" -ge 3 ]]; then
    echo "error: installing the Linux desktop build packages failed after $attempt attempts" >&2
    exit 1
  fi
  sleep $((15 * attempt))
  attempt=$((attempt + 1))
done

pkg-config --modversion webkit2gtk-4.1 >/dev/null
echo "Linux desktop build dependencies installed (webkit2gtk-4.1 $(pkg-config --modversion webkit2gtk-4.1))"
