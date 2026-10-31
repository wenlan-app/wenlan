#!/usr/bin/env bash
# UserPromptSubmit hook (UX-CHAT-1/2): ask the local Wenlan daemon which of
# the person's earlier decisions bear on this prompt (POST /api/check/context)
# and print the daemon's block, which Claude Code hands the AI as context:
#   Past decisions in Wenlan that bear on this (check before contradicting them):
#   - 2026-10-12: Use libSQL instead of a graph database. (wenlan://page/...)
# The daemon picks the decisions and writes the text; this hook only carries
# the prompt there and the block back.
#
# Silent (no output, exit 0) when the daemon is not running, does not answer
# within one second, finds nothing, or python3 is missing. Never blocks the
# prompt.
set -u

command -v python3 >/dev/null 2>&1 || exit 0

input=""
if [ ! -t 0 ]; then
  IFS= read -r -t 2 -d '' input 2>/dev/null || true
fi
[ -n "$input" ] || exit 0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESOLVE="${CLAUDE_PLUGIN_ROOT:-$SCRIPT_DIR/..}/scripts/resolve-space.sh"
HOST="${WENLAN_HOST:-http://127.0.0.1:7878}"

cwd=$(printf '%s' "$input" | python3 -I -c '
import json, sys
try:
    print(json.load(sys.stdin).get("cwd") or "")
except Exception:
    pass
' 2>/dev/null)
space=""
if [ -n "$cwd" ] && [ -f "$RESOLVE" ]; then
  space=$(bash "$RESOLVE" --cwd "$cwd" 2>/dev/null | head -n 1 | cut -f1)
fi

printf '%s' "$input" | WENLAN_CHECK_HOST="$HOST" WENLAN_CHECK_SPACE="$space" python3 -I -c '
import json, os, sys, urllib.request

try:
    hook = json.load(sys.stdin)
except Exception:
    sys.exit(0)
prompt = hook.get("prompt") or ""
if not isinstance(prompt, str) or not prompt.strip():
    sys.exit(0)

request = {"prompt": prompt[:20000]}
sid = hook.get("session_id") or ""
if sid and len(sid) <= 128 and all(c.isascii() and (c.isalnum() or c in "._-") for c in sid):
    request["session"] = "claude-code:" + sid
space = os.environ.get("WENLAN_CHECK_SPACE", "").strip()
if space:
    request["space"] = space

try:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    req = urllib.request.Request(
        os.environ["WENLAN_CHECK_HOST"].rstrip("/") + "/api/check/context",
        data=json.dumps(request).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with opener.open(req, timeout=1) as response:
        body = json.load(response)
    context = body.get("context") or ""
except Exception:
    sys.exit(0)
if isinstance(context, str) and context.strip():
    sys.stdout.write(context.rstrip("\n") + "\n")
' 2>/dev/null
exit 0
