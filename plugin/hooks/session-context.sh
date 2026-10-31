#!/usr/bin/env bash
# SessionStart hook: tell Wenlan which Claude Code session this is, so what the
# agent captures, writes or hands off links back to this conversation.
#   1. Appends `export WENLAN_SESSION=claude-code:<id>` to $CLAUDE_ENV_FILE
#      (when Claude Code provides one), so the `wenlan` CLI and the MCP server
#      the agent starts from Bash see it. Plain assignment: PowerShell reads it too.
#   2. Prints one context line so the agent passes `session` on `capture`,
#      `write_page` and `update_brief`.
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
exit 0
