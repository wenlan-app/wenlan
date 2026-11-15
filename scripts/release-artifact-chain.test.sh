#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

release_targets="scripts/release_targets.py"
cli_runner="crates/wenlan-cli/npm/run.js"

# Every archive the npm runner can download must be one the release produces,
# under the same name; a rename on either side fails here, not at a first run.
matrix="$(python3 "$release_targets" matrix)" || fail "could not read the release target inventory"

for expected_asset in \
  wenlan-darwin-arm64.tar.gz \
  wenlan-linux-x64.tar.gz \
  wenlan-linux-arm64.tar.gz \
  wenlan-windows-x64.zip; do
  artifact="${expected_asset%.tar.gz}"
  artifact="${artifact%.zip}"
  case "$matrix" in
    *"\"artifact_name\":\"${artifact}\""*) ;;
    *) fail "canonical release target inventory does not produce ${artifact}" ;;
  esac
  grep -qF "asset: \"${expected_asset}\"" "$cli_runner" \
    || fail "npm wenlan runner does not download ${expected_asset}"
done

runner_assets="$(grep -c 'asset: "wenlan-' "$cli_runner")" || fail "npm wenlan runner names no release archive"
[ "$runner_assets" -eq 4 ] \
  || fail "npm wenlan runner names ${runner_assets} archives; this test checks 4"

if grep -q '\${name}-\${TARGET}' "$cli_runner"; then
  fail "npm wenlan runner still downloads per-binary target-name assets"
fi

echo "PASS: npm wenlan runner consumes the release.yml artifact for each supported computer"
