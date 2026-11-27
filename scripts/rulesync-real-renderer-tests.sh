#!/usr/bin/env bash
# Run the #[ignore]d tests that need the real, pinned rulesync renderer.
#
# The ordinary suites render rules with a fixture renderer. These tests call
# the pinned release binary instead, so they prove that the real renderer keeps
# Wenlan's shared-rules markers, the team -> personal -> project order, and
# that a person's home sync writes no team block.
#
# The pin (version, asset name, sha256) is read from rulesync_renderer.rs, so
# this script never carries a second copy of it. The binary is downloaded from
# the rulesync GitHub release and refused unless its sha256 matches the pin.
# Set WENLAN_TEST_RULESYNC_BINARY to an already downloaded copy to skip the
# download; it is still checked against the pin.
#
# Every test runs with a throwaway HOME, CODEX_HOME and WENLAN_DATA_DIR and
# with WENLAN_NO_AUTOSTART=1. The run fails unless every listed test reports ok,
# so a renamed or filtered-out test cannot pass as "0 tests ran".
#
# drift_guard.rs keeps the two lists below equal to the ignored tests that read
# WENLAN_TEST_RULESYNC_BINARY on Linux (minus the ones excluded there).
set -euo pipefail

CORE_TESTS=(
  rulesync_renderer::tests::render_session_reuses_only_its_verified_snapshot
  rulesync_renderer::tests::renders_rules_with_local_pinned_binary
)
SERVER_TESTS=(
  rulesync_checkout::tests::rulesync_checkout_claude_and_codex_roots_follow_team_personal_project_order
  rulesync_routes::tests::rulesync_home_sync_with_team_layers_and_the_pinned_renderer_writes_no_team_block
  rulesync_routes::tests::rulesync_more_personal_targets_round_trip_with_pinned_binary
  rulesync_routes::tests::rulesync_personal_files_use_pinned_rendering_and_byte_copies
  rulesync_routes::tests::rulesync_preview_byte_copies_a_package_rejected_by_the_pin
)

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
renderer="$root/crates/wenlan-core/src/rulesync_renderer.rs"

case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) asset=rulesync-linux-x64 ;;
  Linux/aarch64 | Linux/arm64) asset=rulesync-linux-arm64 ;;
  Darwin/arm64) asset=rulesync-darwin-arm64 ;;
  Darwin/x86_64) asset=rulesync-darwin-x64 ;;
  *)
    echo "rulesync-real-renderer-tests: no rulesync pin for $(uname -s)/$(uname -m)" >&2
    exit 1
    ;;
esac

version="$(sed -n 's/^pub const PINNED_VERSION: &str = "\([0-9.]*\)";$/\1/p' "$renderer")"
# The digest is the quoted line right after the asset name inside pin_for.
sha256="$(awk -v name="\"$asset\"," '
  found { gsub(/[ ",]/, ""); print; exit }
  $1 == name { found = 1 }
' "$renderer")"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || ! [[ "$sha256" =~ ^[0-9a-f]{64}$ ]]; then
  echo "rulesync-real-renderer-tests: could not read the $asset pin from $renderer" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

binary="${WENLAN_TEST_RULESYNC_BINARY:-}"
if [ -z "$binary" ]; then
  mkdir "$work/bin"
  binary="$work/bin/$asset"
  url="https://github.com/dyoshikawa/rulesync/releases/download/v$version/$asset"
  echo "Downloading rulesync $version ($asset)"
  curl -fsSL --retry 3 --retry-delay 5 -o "$binary" "$url"
  chmod +x "$binary"
fi
# Tests run from each package's folder, so pass an absolute path.
binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"

if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$binary" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$binary" | cut -d' ' -f1)"
fi
if [ "$actual" != "$sha256" ]; then
  echo "rulesync-real-renderer-tests: $binary has sha256 $actual, the pin is $sha256" >&2
  exit 1
fi
echo "rulesync $version sha256 $actual matches the pin"

# Keep the toolchain where it is before HOME moves to the scratch folder.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
mkdir -p "$work/home/codex" "$work/data"

run_suite() {
  local package="$1"
  shift
  local log="$work/$package.log"
  env -u WENLAN_SESSION \
    HOME="$work/home" \
    CODEX_HOME="$work/home/codex" \
    WENLAN_DATA_DIR="$work/data" \
    WENLAN_NO_AUTOSTART=1 \
    WENLAN_TEST_RULESYNC_BINARY="$binary" \
    cargo test --manifest-path "$root/Cargo.toml" -p "$package" --lib -- --include-ignored --exact "$@" 2>&1 |
    tee "$log"
  local missing=0
  for name in "$@"; do
    if ! grep -qxF "test $name ... ok" "$log"; then
      echo "rulesync-real-renderer-tests: $package test $name did not run and pass" >&2
      missing=1
    fi
  done
  return "$missing"
}

# The lists above must still match the source: a test added only in
# wenlan-server would otherwise skip this lane without any core change.
run_suite wenlan-core \
  drift_guard::rulesync_real_renderer_test::ci_runs_every_portable_real_renderer_test \
  "${CORE_TESTS[@]}"
run_suite wenlan-server "${SERVER_TESTS[@]}"
echo "All $(( ${#CORE_TESTS[@]} + ${#SERVER_TESTS[@]} )) real-renderer tests passed"
