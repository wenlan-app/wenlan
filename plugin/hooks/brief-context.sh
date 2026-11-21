#!/usr/bin/env bash
# SessionStart: print the current Space Brief's one-line items and ids. Silent
# when Wenlan is down or the folder has no Space. Output is capped near 2k
# tokens (8k characters) so starting work never floods the context.
set -u
export WENLAN_NO_AUTOSTART=1
command -v python3 >/dev/null 2>&1 || exit 0

input=""
if [ ! -t 0 ]; then
  IFS= read -r -t 2 -d '' input 2>/dev/null || true
fi
[ -n "$input" ] || exit 0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -n "${CLAUDE_PLUGIN_ROOT:-}" ]; then
  RESOLVE="$CLAUDE_PLUGIN_ROOT/scripts/resolve-space.sh"
elif [ -f "$SCRIPT_DIR/../bin/resolve-space.sh" ]; then
  RESOLVE="$SCRIPT_DIR/../bin/resolve-space.sh"
else
  RESOLVE="$SCRIPT_DIR/resolve-space.sh"
fi
HOST="${WENLAN_HOST:-http://127.0.0.1:7878}"
cwd=$(printf '%s' "$input" | python3 -I -c 'import json,sys
try: print(json.load(sys.stdin).get("cwd") or "")
except Exception: pass' 2>/dev/null)
codex=0
if [ "${WENLAN_HOOK_HOST:-}" = "codex" ]; then
  codex=1
fi
space="${WENLAN_SPACE:-}"
if [ -z "$space" ] && [ -n "$cwd" ] && [ -f "$RESOLVE" ]; then
  space=$(WENLAN_RESOLVE="$RESOLVE" WENLAN_RESOLVE_CWD="$cwd" python3 -I -c '
import os, subprocess
try:
    result = subprocess.run(
        ["bash", os.environ["WENLAN_RESOLVE"], "--cwd", os.environ["WENLAN_RESOLVE_CWD"]],
        capture_output=True, text=True, timeout=2, check=False)
    line = result.stdout.splitlines()[0] if result.stdout.splitlines() else ""
    print(line.split("\t", 1)[0])
except Exception:
    pass
' 2>/dev/null)
fi
[ -n "$space" ] || exit 0

WENLAN_BRIEF_HOST="$HOST" WENLAN_BRIEF_SPACE="$space" WENLAN_BRIEF_CODEX="$codex" python3 -I - <<'PY' 2>/dev/null || true
import json, os, urllib.request
try:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    req = urllib.request.Request(
        os.environ["WENLAN_BRIEF_HOST"].rstrip("/") + "/api/brief",
        data=json.dumps({"topic": None, "space": os.environ["WENLAN_BRIEF_SPACE"]}).encode(),
        headers={"content-type": "application/json"}, method="POST")
    with opener.open(req, timeout=1) as response:
        body = json.load(response)
except Exception:
    raise SystemExit(0)
brief = body.get("brief") or {}
items = list(brief.get("active") or []) + list(brief.get("backlog") or [])
if not items:
    raise SystemExit(0)
lines = ["Wenlan Brief (one line per thing; use its id to read details):"]
for item in items:
    item_id = str(item.get("id") or "").strip()
    text = " ".join(str(item.get("text") or "").split())
    if item_id and text:
        label = " [filled in by Wenlan / Wenlan 補的; not confirmed]" if item.get("filled_by_wenlan") else ""
        lines.append(f"- {item_id}: {text}{label}")
out = "\n".join(lines)
out = out[:8000]
if os.environ.get("WENLAN_BRIEF_CODEX") == "1":
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": "SessionStart", "additionalContext": out
    }}))
else:
    print(out)
PY
exit 0
