#!/usr/bin/env bash
# Bash tests for session-rules.sh: the SessionStart hook prints the approved
# working rules, and nothing when there are none or the daemon is away. A
# stand-in daemon on a free loopback port answers; the real daemon's port is
# never used.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/session-rules.sh"
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

# Stand-in daemon: /api/skills/rules answers general.json; with ?space=Known
# it answers space.json; any other Space is a 404. Each request path is logged.
cat >"$tmpbase/server.py" <<'PY'
import http.server, os, sys
base = sys.argv[1]
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(os.path.join(base, "requests"), "a") as f:
            f.write(self.path + " " + (self.headers.get("x-agent-name") or "") + "\n")
        if "space=" in self.path and "space=Known" not in self.path:
            self.send_response(404)
            self.end_headers()
            return
        name = "space.json" if "space=Known" in self.path else "general.json"
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

run() { env -u WENLAN_SPACE -u WENLAN_DEFAULT_SPACE WENLAN_HOST="$host" "$@" bash "$HOOK" </dev/null; }

printf '{"lines": [{"text": "Discuss the\\napproach first.", "general": true}]}' >"$tmpbase/general.json"
printf '{"lines": [{"text": "Run the tests."}, {"text": "Discuss the approach first.", "general": true}]}' >"$tmpbase/space.json"

out="$(run env)"
check "no Space: general rules, one per line" \
    "- Discuss the approach first. (every Space)" "$(printf '%s\n' "$out" | sed -n 2p)"
case "$out" in "[wenlan] How the user works"*) got=yes ;; *) got=no ;; esac
check "starts by saying whose rules these are" yes "$got"
check "the read names the tool" "/api/skills/rules claude-code" "$(tail -n 1 "$tmpbase/requests")"

out="$(run env WENLAN_DEFAULT_SPACE=Known)"
check "a named Space: its own rule first" "- Run the tests." "$(printf '%s\n' "$out" | sed -n 2p)"
check "a named Space: general rule after" \
    "- Discuss the approach first. (every Space)" "$(printf '%s\n' "$out" | sed -n 3p)"

out="$(run env WENLAN_SPACE=Gone)"
check "an unknown Space falls back to the general rules" \
    "- Discuss the approach first. (every Space)" "$(printf '%s\n' "$out" | sed -n 2p)"

printf '{"lines": []}' >"$tmpbase/general.json"
check "no approved rules prints nothing" "" "$(run env)"

printf '{"lines": [{"text": "x", "general": true}]}' >"$tmpbase/general.json"
check "WENLAN_SESSION_RULES=0 prints nothing" "" "$(run env WENLAN_SESSION_RULES=0)"

kill "$server_pid" 2>/dev/null
server_pid=""
check "an unreachable daemon prints nothing" "" "$(run env)"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
