#!/usr/bin/env bash
# Dispatch the wenlan MCP server.
#
# Resolution order (most specific first). Every override is held to the same
# test as the installed binaries below: a regular file (`-f`) that is
# executable (`-x`), because a directory is "executable" too.
#   1. WENLAN_MCP_DEV_BIN env var (primary) or ORIGIN_MCP_DEV_BIN (fallback) —
#      the same first slot the Wenlan app's own resolver gives it
#      (app/src/mcp_config.rs). Requires Claude Code to inherit the var at
#      startup. Accepts both names for backward compatibility.
#   2. Sibling file `scripts/wenlan-mcp.local` next to this script — typically a
#      symlink to a locally-built wenlan-mcp binary. Filesystem-based so it
#      survives plugin reloads that don't re-read settings.json env.
#   3. An installed binary, in this order (mirrors the order the Wenlan app
#      uses when it writes its own MCP entry, app/src/mcp_config.rs):
#        a. The Wenlan desktop app's bundled copy: first the path the app
#           records for itself on every launch (~/.wenlan/bin/wenlan-mcp.path),
#           then Wenlan.app/Contents/MacOS/wenlan-mcp under /Applications or
#           ~/Applications (WENLAN_APP_DIRS, colon-separated, overrides both),
#           then wenlan-mcp.exe in the Windows installer's folders.
#        b. ~/.wenlan/bin/wenlan-mcp — the path install.sh places binaries at.
#        c. ~/.cargo/bin/wenlan-mcp — `cargo install`.
#        d. `wenlan-mcp` on PATH, for example from a package manager.
#      Preferred over npx because (a) it's already on disk so MCP host
#      handshake is instant, and (b) it sidesteps the EPERM class of npx
#      failures when ~/.npm/_cacache contains root-owned files left over
#      from older npm versions (npx exits before responding to initialize,
#      MCP host then waits 30s and times out).
#   4. npx -y wenlan-mcp@<exact version> — LAST resort for users with no
#      installed binary at all (it downloads on first use and needs network and
#      a working npm cache). The Claude directory rejects ranges and @latest,
#      so this is a literal pin that the release train rewrites
#      (scripts/bump-version.sh) and checks (scripts/validate-versions.sh).

# Don't enable `set -u` here: if Claude Code (or any MCP host) invokes the
# script through a shell that doesn't populate BASH_SOURCE, `set -u` halts
# before we even get to the npx fallback. Fall back to $0 instead.
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd -P)"
local_bin="${here}/wenlan-mcp.local"
agent_name="${WENLAN_MCP_AGENT_NAME:-claude-code}"

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

# wenlan-mcp-pin: kept in lockstep by scripts/bump-version.sh
exec npx -y wenlan-mcp@0.20.2 --agent-name "${agent_name}" "$@"
