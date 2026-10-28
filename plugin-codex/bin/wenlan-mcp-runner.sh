#!/usr/bin/env bash
# Dispatch the Wenlan MCP server for Codex.
#
# Resolution order (most specific first). Every override is held to the same
# test as the installed binaries below: a regular file (`-f`) that is
# executable (`-x`), because a directory is "executable" too.
#   1. WENLAN_MCP_DEV_BIN env var, for local development — the same first slot
#      the Wenlan app's own resolver gives it (app/src/mcp_config.rs).
#   2. Sibling file `bin/wenlan-mcp.local` next to this script.
#   3. An installed binary, in this order (mirrors the order the Wenlan app
#      uses when it writes its own MCP entry, app/src/mcp_config.rs):
#        a. The Wenlan desktop app's bundled copy: first the path the app
#           records for itself on every launch (~/.wenlan/bin/wenlan-mcp.path),
#           then Wenlan.app/Contents/MacOS/wenlan-mcp under /Applications or
#           ~/Applications (WENLAN_APP_DIRS, colon-separated, overrides both),
#           then wenlan-mcp.exe in the Windows installer's folders.
#        b. ~/.wenlan/bin/wenlan-mcp, installed by install.sh.
#        c. ~/.cargo/bin/wenlan-mcp — `cargo install`.
#        d. `wenlan-mcp` on PATH, for example from a package manager.
#      Preferred over npx: already on disk (instant MCP handshake) and it
#      avoids npx's cache-permission failures.
#   4. npx -y wenlan-mcp@^<.codex-plugin/plugin.json version>, LAST-resort
#      package fallback (downloads on first use). The version is derived from
#      the sibling plugin.json (the single source of truth kept on the release
#      train) rather than a hardcoded pin that silently drifts — same approach
#      as the Claude runner.
#
# The explicit agent name is a Codex plugin requirement: stdio MCP clients may
# send a client name during initialize, but the fallback must not mislabel
# Codex captures as another client.

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd -P)"
local_bin="${here}/wenlan-mcp.local"
agent_name="${WENLAN_MCP_AGENT_NAME:-codex}"

dev_bin="${WENLAN_MCP_DEV_BIN:-${ORIGIN_MCP_DEV_BIN:-}}"
if [ -n "${dev_bin}" ] && [ -f "${dev_bin}" ] && [ -x "${dev_bin}" ]; then
  exec "${dev_bin}" --agent-name "${agent_name}" "$@"
fi

if [ -f "${local_bin}" ] && [ -x "${local_bin}" ]; then
  exec "${local_bin}" --agent-name "${agent_name}" "$@"
fi

# Installed binaries. `-f` as well as `-x`: a directory is "executable" too.
candidates=()

# The path the Wenlan app records for its own bundled wenlan-mcp every time it
# starts (app/src/mcp_config.rs, `record_app_wenlan_mcp_path`), so an app run
# from anywhere (outside /Applications, a custom Windows install folder) is
# found. One line, read as data and never sourced; a stale path (the app was
# moved or removed) fails the checks below like any other candidate.
recorded_path_file="${HOME}/.wenlan/bin/wenlan-mcp.path"
if [ -f "${recorded_path_file}" ]; then
  recorded=""
  IFS= read -r recorded < "${recorded_path_file}" || true
  recorded="${recorded%$'\r'}"
  recorded="${recorded//\\//}"
  if [ -n "${recorded}" ]; then
    candidates+=("${recorded}")
  fi
fi

app_dirs="${WENLAN_APP_DIRS:-/Applications:${HOME}/Applications}"
IFS=":" read -r -a app_dir_list <<< "${app_dirs}"
for app_dir in "${app_dir_list[@]}"; do
  if [ -n "${app_dir}" ]; then
    candidates+=("${app_dir}/Wenlan.app/Contents/MacOS/wenlan-mcp")
  fi
done
# The Windows installer's folders, for an app too old to record its path:
# the per-user default (%LOCALAPPDATA%\Programs\Wenlan), the default before
# that move (%LOCALAPPDATA%\Wenlan), and a machine-wide install. Git Bash
# passes these through as Windows paths; forward slashes are what it accepts.
for win_dir in "${LOCALAPPDATA:+${LOCALAPPDATA}/Programs}" "${LOCALAPPDATA:-}" \
  "${PROGRAMFILES:-${ProgramFiles:-}}"; do
  if [ -n "${win_dir}" ]; then
    candidates+=("${win_dir//\\//}/Wenlan/wenlan-mcp.exe")
  fi
done
candidates+=("${HOME}/.wenlan/bin/wenlan-mcp" "${HOME}/.wenlan/bin/wenlan-mcp.exe")
candidates+=("${HOME}/.cargo/bin/wenlan-mcp" "${HOME}/.cargo/bin/wenlan-mcp.exe")
for candidate in "${candidates[@]}"; do
  if [ -f "${candidate}" ] && [ -x "${candidate}" ]; then
    exec "${candidate}" --agent-name "${agent_name}" "$@"
  fi
done

# A `wenlan-mcp` on PATH. Absolute paths only: a bare name from `command -v`
# (a function or alias) would be resolved against the current directory.
path_bin="$(command -v wenlan-mcp 2>/dev/null || true)"
case "${path_bin}" in
  /*)
    if [ -f "${path_bin}" ] && [ -x "${path_bin}" ]; then
      exec "${path_bin}" --agent-name "${agent_name}" "$@"
    fi
    ;;
esac

# Derive the npm version from the sibling plugin.json (single source of truth on
# the release train) so the fallback can't drift from a hardcoded pin; @latest if
# it can't be read. Strip any pre-release/build suffix (e.g. "0.17.0+codex").
# ponytail: sed-parse the one "version" key — no python/jq dep in the MCP host shell.
plugin_json="${here}/../.codex-plugin/plugin.json"
ver="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${plugin_json}" 2>/dev/null | head -1)"
ver="${ver%%+*}"
ver="${ver%%-*}"
if [ -n "${ver}" ]; then
  exec npx -y "wenlan-mcp@^${ver}" --agent-name "${agent_name}" "$@"
fi
exec npx -y wenlan-mcp@latest --agent-name "${agent_name}" "$@"
