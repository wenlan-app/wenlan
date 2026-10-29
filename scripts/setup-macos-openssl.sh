#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail

setup_macos_openssl() {
  local host_os
  host_os="$(uname -s)" || return 1
  [[ "$host_os" == "Darwin" ]] || return 0
  command -v brew >/dev/null || { echo 'Homebrew is required for macOS OpenSSL 3.' >&2; return 1; }

  local prefix version entry probe
  if ! prefix="$(brew --prefix openssl@3)" || [[ ! -x "$prefix/bin/openssl" ]]; then
    HOMEBREW_NO_AUTO_UPDATE=1 brew install openssl@3 || return 1
    prefix="$(brew --prefix openssl@3)" || return 1
  fi
  version="$("$prefix/bin/openssl" version)" || return 1
  case "$version" in
    'OpenSSL 3.'*) ;;
    *) echo "Expected OpenSSL 3, got: $version" >&2; return 1 ;;
  esac
  for entry in include/openssl/ssl.h lib/libssl.dylib lib/libcrypto.dylib; do
    [[ -f "$prefix/$entry" ]] || { echo "Missing OpenSSL 3 input: $prefix/$entry" >&2; return 1; }
  done

  # FindOpenSSL reads this hint itself. OPENSSL_DIR belongs to Rust's separate
  # openssl-sys build and does not select cpp-httplib's C++ dependency.
  export OPENSSL_ROOT_DIR="$prefix"
  probe="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/wenlan-openssl.XXXXXXXX")" || return 1
  cat > "$probe/CMakeLists.txt" <<'CMAKE'
cmake_minimum_required(VERSION 3.20)
project(wenlan_openssl_probe C)
find_package(OpenSSL 3...<4 REQUIRED COMPONENTS SSL Crypto)
file(REAL_PATH "$ENV{OPENSSL_ROOT_DIR}" expected_root)
foreach(input OPENSSL_INCLUDE_DIR OPENSSL_SSL_LIBRARY OPENSSL_CRYPTO_LIBRARY)
  file(REAL_PATH "${${input}}" resolved)
  string(FIND "${resolved}" "${expected_root}/" root_position)
  if(NOT root_position EQUAL 0)
    message(FATAL_ERROR "${input} escaped OpenSSL 3 root: ${resolved}")
  endif()
  message(STATUS "Verified ${input}: ${resolved}")
endforeach()
CMAKE
  cmake -S "$probe" -B "$probe/build" || return 1
  if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf 'OPENSSL_ROOT_DIR=%s\n' "$prefix" >> "$GITHUB_ENV"
  fi
  printf 'macOS native OpenSSL: %s (%s)\n' "$prefix" "$version"
}

setup_macos_openssl
