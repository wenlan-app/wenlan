#!/usr/bin/env bash
# SessionStart hook: put the person's approved working rules in front of the
# conversation at its start. A rule the tool has to remember to fetch is read
# about half the time; one that is already in context is followed. Prints the
# rules used in every Space, plus the named Space's own when WENLAN_SPACE or
# WENLAN_DEFAULT_SPACE names one. Prints nothing when the daemon is not
# reachable, the setting is off, or there are no approved rules.
# WENLAN_SESSION_RULES=0 turns it off.

set -u

[ "${WENLAN_SESSION_RULES:-1}" = "0" ] && exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HOST="${WENLAN_HOST:-http://127.0.0.1:7878}"
SPACE="${WENLAN_SPACE:-${WENLAN_DEFAULT_SPACE:-}}"

WENLAN_RULES_HOST="$HOST" WENLAN_RULES_SPACE="$SPACE" python3 -I -c '
import json, os, sys, urllib.parse, urllib.request

host = os.environ.get("WENLAN_RULES_HOST", "").rstrip("/")
space = os.environ.get("WENLAN_RULES_SPACE", "").strip()
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
MAX = 30

def get(path):
    request = urllib.request.Request(host + path, headers={"x-agent-name": "claude-code"})
    with opener.open(request, timeout=2) as response:
        return json.load(response)

lines = None
if space:
    try:
        lines = get("/api/skills/rules?space=" + urllib.parse.quote(space)).get("lines")
    except Exception:
        lines = None
if lines is None:
    try:
        lines = get("/api/skills/rules").get("lines")
    except Exception:
        sys.exit(0)

rules = []
for line in lines or []:
    text = " ".join(str(line.get("text") or "").split())
    if not text:
        continue
    scope = " (every Space)" if line.get("general") else ""
    rules.append(f"- {text}{scope}")
if not rules:
    sys.exit(0)

print(
    "[wenlan] How the user works, taken from their own pages in Wenlan. Follow "
    "these in this conversation. Where two disagree, the one not marked "
    "(every Space) wins."
)
print("\n".join(rules[:MAX]))
if len(rules) > MAX:
    print(f"- ({len(rules) - MAX} more: read them with the Wenlan read_space_rules tool.)")
' 2>/dev/null
exit 0
