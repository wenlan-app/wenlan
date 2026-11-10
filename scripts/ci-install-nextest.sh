#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Fallback for a failed taiki-e/install-action cargo-nextest download: fetch
# nextest's own prebuilt release, with retries, into Cargo's bin directory.
set -euo pipefail

case "${RUNNER_OS:-$(uname -s)}" in
  Linux) platform=linux ;;
  macOS | Darwin) platform=mac ;;
  Windows | MINGW* | MSYS*) platform=windows-tar ;;
  *) echo "ci-install-nextest: unsupported runner OS ${RUNNER_OS:-unknown}" >&2; exit 1 ;;
esac

bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bin_dir"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bash "$script_dir/ci-retry.sh" 3 180 \
  bash -c 'curl --fail --location --silent --show-error --retry 3 "https://get.nexte.st/latest/$1" | tar zxf - -C "$2"' \
  _ "$platform" "$bin_dir"
"$bin_dir/cargo-nextest" nextest --version
