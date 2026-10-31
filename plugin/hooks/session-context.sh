#!/usr/bin/env bash
# SessionStart hook: tell Wenlan which Claude Code session this is, so what the
# agent captures, writes or hands off links back to this conversation.
#   1. Appends `export WENLAN_SESSION=claude-code:<id>` to $CLAUDE_ENV_FILE
#      (when Claude Code provides one), so the `wenlan` CLI and the MCP server
#      the agent starts from Bash see it. Plain assignment: PowerShell reads it too.
#   2. Prints one context line so the agent passes `session` on `capture`,
#      `write_page` and `update_brief`.
#   3. Prints the person's page on how AI should work with them (UX-HOW-1,
#      GET /api/pages/instructions), so the agent reads it before starting.
#      The first read writes it once from the person's memories. Skipped
#      silently when the daemon is not running or python3 is missing.
# Reads the hook JSON from stdin without jq. Never blocks (always exit 0) and
# stays silent when the session id is missing or not a plain id.
set -u

input=""
# `read -t` bounds the wait if stdin is a pipe nobody closes; -d '' reads to EOF.
if [ ! -t 0 ]; then
  IFS= read -r -t 2 -d '' input 2>/dev/null || true
fi
[ -n "$input" ] || exit 0

# Flatten so the match works on pretty-printed JSON too, then take the string
# value of "session_id".
id=$(printf '%s' "$input" | tr -d '\r\n' |
  sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)

# The daemon accepts 1 to 128 of [A-Za-z0-9._-]; anything else is not ours to send.
case "$id" in
  '' | *[!A-Za-z0-9._-]*) exit 0 ;;
esac
[ "${#id}" -le 128 ] || exit 0

session="claude-code:$id"

if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  { printf 'export WENLAN_SESSION=%s\n' "$session" >>"$CLAUDE_ENV_FILE"; } 2>/dev/null || true
fi

printf 'Wenlan session: %s. Pass it as session when you call capture, write_page or update_brief.\n' "$session"

if command -v python3 >/dev/null 2>&1; then
  WENLAN_CHECK_HOST="${WENLAN_HOST:-http://127.0.0.1:7878}" python3 -I -c '
import json, os, sys, urllib.request
try:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    url = os.environ["WENLAN_CHECK_HOST"].rstrip("/") + "/api/pages/instructions"
    with opener.open(url, timeout=3) as response:
        page = (json.load(response) or {}).get("page") or {}
    title, link, content = page.get("title"), page.get("link"), (page.get("content") or "").strip()
    if title and link and content:
        print()
        print("How the person wants AI to work with them: %s (v%s, %s)" % (title, page.get("version"), link))
        print("Follow it. To change it, propose an edit with write_page; the person accepts or dismisses it.")
        print()
        print(content)
except Exception:
    pass
' 2>/dev/null </dev/null || true
fi
exit 0
