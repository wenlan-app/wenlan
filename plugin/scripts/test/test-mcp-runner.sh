#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Bash 3.2-compatible contract tests for wenlan-mcp-runner.sh resolution order.
#
# Byte-identical copies live in plugin/scripts/test and plugin-codex/bin/test;
# the flavor (Claude vs Codex runner) is detected from the runner itself.
# Everything runs in a temp HOME with a minimal PATH, so a developer's real
# ~/.wenlan, Wenlan.app, ~/.cargo or PATH cannot change the result.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNER_SRC="$SCRIPT_DIR/../wenlan-mcp-runner.sh"
tmpbase="$(mktemp -d "${TMPDIR:-/tmp}/wenlan-runner-test.XXXXXX")"
trap 'rm -rf "$tmpbase"' EXIT

pass=0
fail=0

assert_eq() {
    name="$1"
    expected="$2"
    actual="$3"
    if [ "$expected" = "$actual" ]; then
        printf 'PASS  %s\n' "$name"
        pass=$((pass + 1))
    else
        printf 'FAIL  %s\n  expected: %q\n  actual:   %q\n' \
            "$name" "$expected" "$actual" >&2
        fail=$((fail + 1))
    fi
}

# Both runners pass --agent-name; the default value tells them apart.
if grep -q 'WENLAN_MCP_AGENT_NAME:-codex' "$RUNNER_SRC"; then
    flavor=codex
    agent_args='--agent-name codex '
else
    flavor=claude
    agent_args='--agent-name claude-code '
fi

# A runner copy in a throwaway plugin tree: `here` resolves to the copy's dir,
# so wenlan-mcp.local can be placed beside it, and the Codex runner finds a
# fixture plugin.json at ../.codex-plugin instead of the repo's.
plugin="$tmpbase/plugin"
mkdir -p "$plugin/run" "$plugin/.codex-plugin"
cp "$RUNNER_SRC" "$plugin/run/wenlan-mcp-runner.sh"
printf '{ "version": "9.8.7" }\n' > "$plugin/.codex-plugin/plugin.json"
runner="$plugin/run/wenlan-mcp-runner.sh"

if [ "$flavor" = codex ]; then
    expected_npx='NPX:-y wenlan-mcp@^9.8.7 --agent-name codex'
else
    pin="$(sed -n 's/^exec npx -y \(wenlan-mcp@[^ ]*\) .*/\1/p' "$RUNNER_SRC")"
    expected_npx="NPX:-y $pin --agent-name claude-code"
fi

# make_bin PATH LABEL: an executable that reports which binary ran.
make_bin() {
    mkdir -p "$(dirname "$1")"
    printf '#!/bin/sh\necho "BIN:%s:$*"\n' "$2" > "$1"
    chmod +x "$1"
}

# Fresh sandbox: HOME, app dir, PATH dir with a fake npx.
new_case() {
    case_dir="$(mktemp -d "$tmpbase/case.XXXXXX")"
    home="$case_dir/home"
    apps="$case_dir/Applications"
    path_dir="$case_dir/pathbin"
    mkdir -p "$home" "$apps" "$path_dir"
    printf '#!/bin/sh\necho "NPX:$*"\n' > "$path_dir/npx"
    chmod +x "$path_dir/npx"
    rm -rf "$plugin/run/wenlan-mcp.local"
}

run_runner() {
    env -i HOME="$home" PATH="$path_dir:/usr/bin:/bin" WENLAN_APP_DIRS="$apps" \
        "$@" bash "$runner" tool-arg 2>&1
}

installed="home/.wenlan/bin/wenlan-mcp"
bundle="Applications/Wenlan.app/Contents/MacOS/wenlan-mcp"
cargo_bin="home/.cargo/bin/wenlan-mcp"

new_case
assert_eq 'nothing installed falls through to the npx pin, args forwarded' \
    "$expected_npx tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
make_bin "$case_dir/$bundle" bundle
make_bin "$case_dir/$cargo_bin" cargo
make_bin "$path_dir/wenlan-mcp" path
assert_eq 'the Wenlan.app bundle wins over HOME/.wenlan/bin, cargo, PATH and npx' \
    "BIN:bundle:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
make_bin "$case_dir/$cargo_bin" cargo
make_bin "$path_dir/wenlan-mcp" path
assert_eq 'HOME/.wenlan/bin wins over cargo, PATH and npx' \
    "BIN:installed:${agent_args}tool-arg" "$(run_runner)"

# The path the app records for its own bundled copy, wherever it was run from.
recorded_file="home/.wenlan/bin/wenlan-mcp.path"

new_case
make_bin "$case_dir/Downloads/Wenlan.app/Contents/MacOS/wenlan-mcp" recorded
make_bin "$case_dir/$bundle" bundle
make_bin "$case_dir/$installed" installed
mkdir -p "$(dirname "$case_dir/$recorded_file")"
printf '%s\n' "$case_dir/Downloads/Wenlan.app/Contents/MacOS/wenlan-mcp" \
    > "$case_dir/$recorded_file"
assert_eq 'the path the app recorded wins over the /Applications bundle' \
    "BIN:recorded:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/Program Files/Wenlan/wenlan-mcp.exe" recorded
make_bin "$case_dir/$installed" installed
mkdir -p "$(dirname "$case_dir/$recorded_file")"
printf '%s\r\n' "$case_dir/Program Files/Wenlan/wenlan-mcp.exe" \
    | tr '/' '\\' > "$case_dir/$recorded_file"
assert_eq 'a recorded Windows path (backslashes, CRLF, a space) is found' \
    "BIN:recorded:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
mkdir -p "$(dirname "$case_dir/$recorded_file")"
printf '%s\n' "$case_dir/moved-away/wenlan-mcp" > "$case_dir/$recorded_file"
assert_eq 'a stale recorded path falls through to the next candidate' \
    "BIN:installed:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
mkdir -p "$case_dir/recorded-dir"
printf '%s\n' "$case_dir/recorded-dir" > "$case_dir/$recorded_file"
assert_eq 'a recorded path naming a directory is skipped, not exec-ed' \
    "BIN:installed:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
mkdir -p "$(dirname "$case_dir/$recorded_file")"
printf 'echo SOURCED; %s\n' "$case_dir/x" > "$case_dir/$recorded_file"
assert_eq 'the recorded path file is read as a path, never run as shell' \
    "BIN:installed:${agent_args}tool-arg" "$(run_runner)"

# The Windows installer's folders, for an app that predates the recorded path.
new_case
make_bin "$case_dir/LocalAppData/Programs/Wenlan/wenlan-mcp.exe" win-user
make_bin "$case_dir/LocalAppData/Wenlan/wenlan-mcp.exe" win-old
make_bin "$case_dir/$installed" installed
assert_eq 'LOCALAPPDATA/Programs/Wenlan (the per-user install) wins over HOME/.wenlan/bin' \
    "BIN:win-user:${agent_args}tool-arg" \
    "$(run_runner LOCALAPPDATA="$(printf '%s' "$case_dir/LocalAppData" | tr '/' '\\')")"

new_case
make_bin "$case_dir/LocalAppData/Wenlan/wenlan-mcp.exe" win-old
make_bin "$case_dir/$installed" installed
assert_eq 'LOCALAPPDATA/Wenlan (the older per-user default) is found' \
    "BIN:win-old:${agent_args}tool-arg" "$(run_runner LOCALAPPDATA="$case_dir/LocalAppData")"

new_case
make_bin "$case_dir/Program Files/Wenlan/wenlan-mcp.exe" win-machine
make_bin "$case_dir/$installed" installed
assert_eq 'a machine-wide Program Files/Wenlan install is found' \
    "BIN:win-machine:${agent_args}tool-arg" "$(run_runner PROGRAMFILES="$case_dir/Program Files")"

new_case
make_bin "$case_dir/$bundle" bundle
make_bin "$case_dir/$cargo_bin" cargo
make_bin "$path_dir/wenlan-mcp" path
assert_eq 'the Wenlan.app bundle wins over cargo, PATH and npx' \
    "BIN:bundle:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$cargo_bin" cargo
make_bin "$path_dir/wenlan-mcp" path
assert_eq 'HOME/.cargo/bin wins over PATH and npx' \
    "BIN:cargo:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$path_dir/wenlan-mcp" path
assert_eq 'wenlan-mcp on PATH wins over npx' \
    "BIN:path:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/home/.wenlan/bin/wenlan-mcp.exe" installed-exe
assert_eq 'a Windows-style wenlan-mcp.exe under HOME/.wenlan/bin is found' \
    "BIN:installed-exe:${agent_args}tool-arg" "$(run_runner)"

new_case
mkdir -p "$case_dir/$installed"
printf '#!/bin/sh\necho NOT-A-BINARY\n' > "$case_dir/$installed/wenlan-mcp"
make_bin "$case_dir/$cargo_bin" cargo
assert_eq 'a directory named wenlan-mcp is not executed' \
    "BIN:cargo:${agent_args}tool-arg" "$(run_runner)"

new_case
mkdir -p "$(dirname "$case_dir/$installed")"
printf '#!/bin/sh\necho NO-EXEC-BIT\n' > "$case_dir/$installed"
make_bin "$case_dir/$cargo_bin" cargo
assert_eq 'a non-executable file is skipped' \
    "BIN:cargo:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
make_bin "$case_dir/dev-bin" dev
assert_eq 'WENLAN_MCP_DEV_BIN beats installed binaries' \
    "BIN:dev:${agent_args}tool-arg" "$(run_runner WENLAN_MCP_DEV_BIN="$case_dir/dev-bin")"

new_case
make_bin "$case_dir/$installed" installed
make_bin "$case_dir/dev-bin" dev
make_bin "$plugin/run/wenlan-mcp.local" local
assert_eq 'WENLAN_MCP_DEV_BIN beats the sibling wenlan-mcp.local (the app resolver order)' \
    "BIN:dev:${agent_args}tool-arg" "$(run_runner WENLAN_MCP_DEV_BIN="$case_dir/dev-bin")"

new_case
make_bin "$case_dir/$installed" installed
make_bin "$plugin/run/wenlan-mcp.local" local
assert_eq 'the sibling wenlan-mcp.local beats installed binaries' \
    "BIN:local:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
make_bin "$plugin/run/wenlan-mcp.local" local
mkdir -p "$case_dir/dev-dir"
assert_eq 'a directory as WENLAN_MCP_DEV_BIN is skipped, not exec-ed' \
    "BIN:local:${agent_args}tool-arg" "$(run_runner WENLAN_MCP_DEV_BIN="$case_dir/dev-dir")"

new_case
make_bin "$case_dir/$installed" installed
mkdir -p "$plugin/run/wenlan-mcp.local"
assert_eq 'a directory named wenlan-mcp.local is skipped, not exec-ed' \
    "BIN:installed:${agent_args}tool-arg" "$(run_runner)"

new_case
make_bin "$case_dir/$installed" installed
printf '#!/bin/sh\necho NO-EXEC-BIT\n' > "$case_dir/dev-file"
assert_eq 'a non-executable WENLAN_MCP_DEV_BIN is skipped' \
    "BIN:installed:${agent_args}tool-arg" "$(run_runner WENLAN_MCP_DEV_BIN="$case_dir/dev-file")"

rm -rf "$plugin/run/wenlan-mcp.local"

printf '\n%s passed, %s failed (%s runner)\n' "$pass" "$fail" "$flavor"
[ "$fail" -eq 0 ]
