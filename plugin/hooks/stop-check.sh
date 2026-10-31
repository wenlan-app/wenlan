#!/usr/bin/env bash
# Stop hook (UX-CHAT-1/2): send the AI's latest reply to the local Wenlan
# daemon (POST /api/check). When the reply goes against something the person
# decided earlier, print one JSON object {"systemMessage": "..."} that Claude
# Code shows to the person: each reminder with its date and a wenlan:// link.
# The daemon owns every rule (what counts as a conflict, five a day, no repeat
# in one session); this hook only carries the reply there and the text back.
#
# Silent (no output, exit 0) when the daemon is not running, does not answer
# within one second, finds nothing, or python3 is missing. Never blocks the
# stop.
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

MAX_REPLY = 20000

def text_blocks(content):
    if isinstance(content, str):
        return [content]
    out = []
    for block in content or []:
        if isinstance(block, dict) and block.get("type") == "text":
            out.append(block.get("text") or "")
    return out

def last_reply_from_transcript(path):
    """Text of the assistant turn after the last prompt the person typed."""
    try:
        with open(path, encoding="utf-8") as handle:
            lines = handle.readlines()[-2000:]
    except Exception:
        return ""
    parts = []
    for line in lines:
        try:
            entry = json.loads(line)
        except Exception:
            continue
        kind = entry.get("type")
        content = (entry.get("message") or {}).get("content")
        if kind == "user" and text_blocks(content):
            parts = []
        elif kind == "assistant":
            parts.extend(text_blocks(content))
    return "\n\n".join(p for p in parts if p.strip())

try:
    hook = json.load(sys.stdin)
except Exception:
    sys.exit(0)
if hook.get("stop_hook_active"):
    sys.exit(0)
reply = hook.get("last_assistant_message") or ""
if not isinstance(reply, str) or not reply.strip():
    reply = last_reply_from_transcript(hook.get("transcript_path") or "")
reply = reply.strip()[-MAX_REPLY:]
if not reply:
    sys.exit(0)

request = {"reply": reply}
sid = hook.get("session_id") or ""
if sid and len(sid) <= 128 and all(c.isascii() and (c.isalnum() or c in "._-") for c in sid):
    request["session"] = "claude-code:" + sid
space = os.environ.get("WENLAN_CHECK_SPACE", "").strip()
if space:
    request["space"] = space

try:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    req = urllib.request.Request(
        os.environ["WENLAN_CHECK_HOST"].rstrip("/") + "/api/check",
        data=json.dumps(request).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with opener.open(req, timeout=1) as response:
        body = json.load(response)
    lines = [r.get("text", "").strip() for r in body.get("reminders") or []]
    lines = [line for line in lines if line]
except Exception:
    sys.exit(0)
if lines:
    print(json.dumps({"systemMessage": "\n".join(lines)}, ensure_ascii=False))
' 2>/dev/null
exit 0
