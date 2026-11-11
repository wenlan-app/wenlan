#!/usr/bin/env bash
# SessionStart hook: when material in Wenlan is not in any page yet and the
# person left page writing to their AI tool (steeping "On demand"), ask this
# conversation to write a few of those pages, so they get written as a side
# effect of using Claude Code instead of a command the person has to remember.
#
# The pages are written here, in the person's own conversation, by the model
# they are already using. Wenlan never runs Claude Code itself.
#
# Prints one context line, or nothing. Silent (exit 0) when:
#   - WENLAN_SESSION_PAGES=0 (the person turned it off),
#   - steeping is Automatic (Wenlan's own model writes the pages),
#   - nothing is waiting, or every waiting item is already being written,
#   - the daemon does not answer within two seconds, or python3 is missing.
# Never blocks the session.
set -u

[ "${WENLAN_SESSION_PAGES:-1}" = "0" ] && exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HOST="${WENLAN_HOST:-http://127.0.0.1:7878}"

WENLAN_PAGES_HOST="$HOST" python3 -I -c '
import json, os, sys, urllib.request

host = os.environ.get("WENLAN_PAGES_HOST", "").rstrip("/")
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def get(path):
    with opener.open(host + path, timeout=2) as response:
        return json.load(response)

try:
    config = get("/api/config")
    if config.get("background_ai_enabled") is True:
        sys.exit(0)
    listing = get("/api/steep/new?limit=1")
    waiting = int(listing.get("total") or 0) - int(listing.get("held") or 0)
except Exception:
    sys.exit(0)
if waiting <= 0:
    sys.exit(0)

limit = min(3, waiting)
print(
    f"[wenlan] {waiting} item(s) in Wenlan are not in any page yet, and the user "
    f"leaves writing pages to you. Write at most {limit} of them into pages with "
    "the Wenlan steep workflow for new material: in a background agent if you can, "
    "otherwise right after your first reply, so the user’s own request is never "
    "delayed. Then say in one short line which pages you wrote. Skip it if the user "
    "asks you not to."
)
' 2>/dev/null
exit 0
