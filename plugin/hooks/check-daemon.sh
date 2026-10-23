#!/usr/bin/env bash
# SessionStart hook: probe the local Wenlan daemon, register Claude Code with
# it, and surface two issues:
#   1. Daemon not running → point user at /wenlan (it auto-installs).
#   2. Daemon version mismatches the plugin manifest → point user at
#      /wenlan (it upgrades/restarts and verifies the runtime).
# Hook never blocks (always exit 0) and never prints command soup.
set -u

URL="${WENLAN_HEALTH_URL:-http://127.0.0.1:7878/api/health}"
W="${WENLAN_CLI:-$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")}"
PLUGIN_JSON="${CLAUDE_PLUGIN_ROOT:-}/.claude-plugin/plugin.json"

# Print a one-line outbox report when queued or failed writes exist. Silent
# on any error (old CLI without the subcommand, no python3, bad JSON).
report_outbox() {
  [ -x "$W" ] || return 0
  command -v python3 >/dev/null 2>&1 || return 0
  status=$("$W" --format json outbox status 2>/dev/null) || return 0
  printf '%s' "$status" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
    q, f = d.get("queued", 0), d.get("failed", 0)
    if q > 0 or f > 0:
        print(f"[wenlan] {q} handoff note(s) waiting to sync, {f} failed — run \`wenlan outbox status\`.")
except Exception:
    pass
' 2>/dev/null
}

RESP=""
# shellcheck disable=SC2034  # the loop variable is a contract the plugin tests read
for i in 1 2 3; do
  RESP=$(curl -fsS -m 3 "$URL" 2>/dev/null) && break
  sleep 1
done

if [ -z "$RESP" ]; then
  cat <<MSG
[wenlan] Wenlan is not running. Handoff notes are saved locally and sync when it starts; run /wenlan or \`wenlan background on\`.
MSG
  report_outbox
  exit 0
fi

report_outbox

# Installing the plugin is how the user connected Claude Code, so tell the
# daemon, as connecting a tool from the app does (POST /api/agents): its first
# recall then sees the user's pages, not only search results. Idempotent; a
# tool the user edited or turned off in Connections stays as it is. Silent.
case "$URL" in
  */api/health)
    curl -fsS -m 3 -o /dev/null -X POST -H 'Content-Type: application/json' \
      -d '{"name":"claude-code"}' "${URL%/api/health}/api/agents" 2>/dev/null || true
    ;;
esac

# Compare daemon version vs plugin manifest version. Silent unless the
# major.minor differ: the daemon and plugin ride one release train, so a patch
# drift (e.g. 0.13.1 vs 0.13.2) is compatible and must NOT nag every session.
# version-drift.sh strips build metadata (`+g<sha8>`), which must never silence
# the drift check, and prints one `daemon=.. expected=.. release=.. result=..`
# line; `unknown` (manifest unreadable, health gone) stays silent.
DRIFT="${CLAUDE_PLUGIN_ROOT:-}/scripts/version-drift.sh"
[ -r "$DRIFT" ] || exit 0
LINE=$(bash "$DRIFT" --health-url "$URL" --manifest "$PLUGIN_JSON" --ignore-patch 2>/dev/null)
field() { printf '%s\n' "$LINE" | tr ' ' '\n' | sed -n "s/^$1=//p"; }
RESULT=$(field result)
DAEMON_VER=$(field daemon)
EXPECTED_VER=$(field expected)

if [ "$RESULT" = older ] || [ "$RESULT" = newer ]; then
  cat <<MSG
[wenlan] daemon v${DAEMON_VER}, plugin expects v${EXPECTED_VER}.
  Run /wenlan to repair: it updates an older runtime, or says when the plugin cache is stale, then verifies MCP.
MSG
fi

exit 0
