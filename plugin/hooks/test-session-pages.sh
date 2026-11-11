#!/usr/bin/env bash
# Bash tests for session-pages.sh: the SessionStart hook asks the conversation
# to write a few waiting pages only when steeping is On demand and material is
# waiting that no other run holds. A stand-in daemon on a free loopback port
# answers; the real daemon's port is never used.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/session-pages.sh"
tmpbase="$(mktemp -d)"
server_pid=""
cleanup() {
    [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
    rm -rf "$tmpbase"
}
trap cleanup EXIT

pass=0
fail=0

check() { # name, expected, got
    if [ "$2" = "$3" ]; then
        printf 'PASS  %s\n' "$1"; pass=$((pass + 1))
    else
        printf 'FAIL  %s\n  expected: %s\n  got:      %s\n' "$1" "$2" "$3" >&2
        fail=$((fail + 1))
    fi
}

if ! command -v python3 >/dev/null 2>&1; then
    printf 'SKIP  python3 is not installed\n'
    exit 0
fi

# Stand-in daemon: GET /api/config answers $tmpbase/config.json and
# GET /api/steep/new answers $tmpbase/new.json.
cat >"$tmpbase/server.py" <<'PY'
import http.server, os, sys
base = sys.argv[1]
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        name = "config.json" if self.path.startswith("/api/config") else "new.json"
        with open(os.path.join(base, name), "rb") as f:
            data = f.read()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *args):
        pass
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(os.path.join(base, "port"), "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
python3 "$tmpbase/server.py" "$tmpbase" &
server_pid=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$tmpbase/port" ] && break; sleep 0.2; done
port="$(cat "$tmpbase/port")"
host="http://127.0.0.1:$port"

run() { WENLAN_HOST="$host" bash "$HOOK" </dev/null; }

printf '{"background_ai_enabled": false}' >"$tmpbase/config.json"
printf '{"items": [], "total": 12, "held": 2}' >"$tmpbase/new.json"
out="$(run)"
case "$out" in
    *"10 item(s) in Wenlan are not in any page yet"*"at most 3 of them"*) got=yes ;;
    *) got="$out" ;;
esac
check "On demand with waiting material asks for at most three pages" yes "$got"

printf '{"items": [], "total": 2, "held": 0}' >"$tmpbase/new.json"
out="$(run)"
case "$out" in *"at most 2 of them"*) got=yes ;; *) got="$out" ;; esac
check "never asks for more pages than are waiting" yes "$got"

printf '{"items": [], "total": 4, "held": 4}' >"$tmpbase/new.json"
check "silent when every waiting item is already being written" "" "$(run)"

printf '{"items": [], "total": 0}' >"$tmpbase/new.json"
check "silent when nothing is waiting" "" "$(run)"

printf '{"background_ai_enabled": true}' >"$tmpbase/config.json"
printf '{"items": [], "total": 12}' >"$tmpbase/new.json"
check "silent in Automatic, where Wenlan's own model writes pages" "" "$(run)"

printf '{"background_ai_enabled": false}' >"$tmpbase/config.json"
check "silent when the person turned it off" "" "$(WENLAN_SESSION_PAGES=0 WENLAN_HOST="$host" bash "$HOOK" </dev/null)"

check "silent when the daemon is not running" "" "$(WENLAN_HOST=http://127.0.0.1:9 bash "$HOOK" </dev/null)"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
