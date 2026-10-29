#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$repo_root/target"
tmp_root="$(cd "$repo_root/target" && pwd -P)"
tmp="$(mktemp -d "$tmp_root/openssl-env-test.XXXXXXXX")"
cleanup() {
  local resolved
  resolved="$(cd "$tmp" && pwd -P)" || return 1
  case "$resolved" in
    "$tmp_root"/openssl-env-test.*) rm -rf -- "$resolved" ;;
    *) echo "Refusing cleanup outside test root: $resolved" >&2; return 1 ;;
  esac
}
trap cleanup EXIT
mkdir -p "$tmp/bin" "$tmp/probes"
export MOCK_SSL_ROOT="$tmp/openssl 3" MOCK_CALLS="$tmp/calls"
export GITHUB_ENV="$tmp/github-env" RUNNER_TEMP="$tmp/probes"
export CMAKE_PREFIX_PATH='/existing prefix:/another-prefix'
cat > "$tmp/bin/uname" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "${MOCK_OS:-Darwin}"
SH
cat > "$tmp/bin/brew" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'brew %s\n' "$*" >> "$MOCK_CALLS"
case "$*" in
  '--prefix openssl@3') printf '%s\n' "$MOCK_SSL_ROOT" ;;
  'install openssl@3')
    [[ "${MOCK_INSTALL_FAIL:-0}" == 0 ]] || exit 1
    mkdir -p "$MOCK_SSL_ROOT/bin" "$MOCK_SSL_ROOT/include/openssl" "$MOCK_SSL_ROOT/lib"
    cat > "$MOCK_SSL_ROOT/bin/openssl" <<'BIN'
#!/usr/bin/env bash
printf '%s\n' "${MOCK_VERSION:-OpenSSL 3.5.9}"
BIN
    chmod +x "$MOCK_SSL_ROOT/bin/openssl"
    touch "$MOCK_SSL_ROOT/include/openssl/ssl.h" "$MOCK_SSL_ROOT/lib/libssl.dylib" "$MOCK_SSL_ROOT/lib/libcrypto.dylib"
    ;;
  *) exit 1 ;;
esac
SH
cat > "$tmp/bin/cmake" <<'SH'
#!/usr/bin/env bash
printf 'cmake\n' >> "$MOCK_CALLS"
[[ "${MOCK_CMAKE_FAIL:-0}" == 0 ]]
SH
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
helper="$repo_root/scripts/setup-macos-openssl.sh"

# A missing keg is installed once; a populated keg is reused. Paths with
# spaces and the caller's existing CMake prefix list survive unchanged.
for pass in 1 2; do
  : > "$GITHUB_ENV"
  bash -c '. "$1"; [[ "$OPENSSL_ROOT_DIR" == "$MOCK_SSL_ROOT" ]]; [[ "$CMAKE_PREFIX_PATH" == "/existing prefix:/another-prefix" ]]' _ "$helper"
  [[ "$(cat "$GITHUB_ENV")" == "OPENSSL_ROOT_DIR=$MOCK_SSL_ROOT" ]]
done
[[ "$(grep -c '^brew install openssl@3$' "$MOCK_CALLS")" == 1 ]]
[[ "$(grep -c '^cmake$' "$MOCK_CALLS")" == 2 ]]

for failure in version missing_library cmake; do
  : > "$GITHUB_ENV"
  case "$failure" in
    version) export MOCK_VERSION='OpenSSL 4.1.0' ;;
    missing_library) unset MOCK_VERSION; mv "$MOCK_SSL_ROOT/lib/libssl.dylib" "$tmp/libssl.saved" ;;
    cmake) mv "$tmp/libssl.saved" "$MOCK_SSL_ROOT/lib/libssl.dylib"; export MOCK_CMAKE_FAIL=1 ;;
  esac
  if bash "$helper"; then
    echo "OpenSSL setup incorrectly accepted $failure" >&2
    exit 1
  fi
  [[ ! -s "$GITHUB_ENV" ]]
done
unset MOCK_CMAKE_FAIL

# Non-macOS hosts never invoke Homebrew or persist a macOS environment.
: > "$MOCK_CALLS"
: > "$GITHUB_ENV"
MOCK_OS=Linux bash "$helper"
[[ ! -s "$MOCK_CALLS" && ! -s "$GITHUB_ENV" ]]
echo 'macOS OpenSSL setup contracts: PASS'
