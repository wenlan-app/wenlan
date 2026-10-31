#!/usr/bin/env bash
# Bash tests for prompt-context.sh: the UserPromptSubmit hook sends the prompt
# to POST /api/check/context and prints the daemon's context block only when
# it has one. A stand-in daemon on a free loopback port answers; the real
# daemon's port is never used.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/prompt-context.sh"
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

# Stand-in daemon: answers POST /api/check/context with $tmpbase/response.json (or
# sleeps past the hook's one second when $tmpbase/slow exists) and records
# each request body in $tmpbase/requests.
cat >"$tmpbase/server.py" <<'PY'
import http.server, os, sys, time
base = sys.argv[1]
class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        with open(os.path.join(base, "requests"), "ab") as log:
            log.write(self.path.encode() + b" " + body + b"\n")
        if os.path.exists(os.path.join(base, "slow")):
            time.sleep(2)
        with open(os.path.join(base, "response.json"), "rb") as f:
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
python3 -I "$tmpbase/server.py" "$tmpbase" &
server_pid=$!
for _ in $(seq 1 50); do [ -s "$tmpbase/port" ] && break; sleep 0.1; done
HOST="http://127.0.0.1:$(cat "$tmpbase/port")"

run() { # stdin payload
    printf '%s' "$1" | WENLAN_HOST="$HOST" CLAUDE_PLUGIN_ROOT="$SCRIPT_DIR/.." bash "$HOOK"
}

BLOCK='Past decisions in Wenlan that bear on this (check before contradicting them):
- 2026-10-12: Use libSQL instead of a graph database. (wenlan://page/page_abc)'
python3 -I -c 'import json,sys; print(json.dumps({"decisions":[{"decided":"Use libSQL instead of a graph database.","decided_on":"2026-10-12","source":{"memory_id":"mem_x","memory_version":1,"link":"wenlan://page/page_abc"}}],"context":sys.argv[1]+"\n"}))' "$BLOCK" >"$tmpbase/response.json"

out=$(run '{"session_id":"abc-1","prompt":"Should entities move to a graph database?","hook_event_name":"UserPromptSubmit"}')
check 'prints the daemon context block' "$BLOCK" "$out"
sent=$(tail -n 1 "$tmpbase/requests")
check 'posts to /api/check/context' '/api/check/context' "${sent%% *}"
check 'sends the prompt and the session' \
    '{"prompt": "Should entities move to a graph database?", "session": "claude-code:abc-1"}' "${sent#* }"

printf '{"decisions":[]}' >"$tmpbase/response.json"
out=$(run '{"session_id":"abc-1","prompt":"Rename the helper."}')
check 'no decisions prints nothing' "" "$out"

printf '{"decisions":[],"context":"late"}' >"$tmpbase/response.json"
touch "$tmpbase/slow"
start=$(date +%s)
out=$(run '{"prompt":"Should entities move to a graph database?"}')
rc=$?
elapsed=$(( $(date +%s) - start ))
rm -f "$tmpbase/slow"
check 'slow daemon prints nothing' "" "$out"
check 'slow daemon exits 0' "0" "$rc"
[ "$elapsed" -le 3 ] && fast=yes || fast=no
check 'slow daemon does not hold the prompt' yes "$fast"

for case_name in 'daemon down' 'empty stdin' 'not json' 'empty prompt'; do
    case "$case_name" in
        'daemon down') payload='{"prompt":"Should entities move to a graph database?"}'; host='http://127.0.0.1:9' ;;
        'empty stdin') payload=''; host="$HOST" ;;
        'not json') payload='garbage'; host="$HOST" ;;
        'empty prompt') payload='{"prompt":"  "}'; host="$HOST" ;;
    esac
    out=$(printf '%s' "$payload" | WENLAN_HOST="$host" bash "$HOOK")
    rc=$?
    check "$case_name prints nothing" "" "$out"
    check "$case_name exits 0" "0" "$rc"
done

out=$(WENLAN_HOST="$HOST" bash "$HOOK" </dev/null)
check 'closed stdin prints nothing' "" "$out"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
