#!/usr/bin/env bash
# Bash tests for stop-check.sh: the Stop hook sends the latest reply to
# POST /api/check and prints {"systemMessage": ...} only when the daemon
# returns reminders. A stand-in daemon on a free loopback port answers; the
# real daemon's port is never used.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/stop-check.sh"
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

# Stand-in daemon: answers POST /api/check with $tmpbase/response.json (or
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

REMINDER='Earlier decision (2026-10-12): Use libSQL instead of a graph database. wenlan://page/page_abc'
printf '{"reminders":[{"seq":1,"text":"%s","decided":"Use libSQL instead of a graph database.","decided_on":"2026-10-12","reply_excerpt":"switch","option":"graph database","source":{"memory_id":"mem_x","memory_version":1,"link":"wenlan://page/page_abc"}}],"remaining_today":4}' "$REMINDER" >"$tmpbase/response.json"

out=$(run '{"session_id":"abc-1","last_assistant_message":"Let us switch to a graph database.","stop_hook_active":false}')
check 'prints the reminder as a systemMessage' "{\"systemMessage\": \"$REMINDER\"}" "$out"
sent=$(tail -n 1 "$tmpbase/requests")
check 'posts to /api/check' '/api/check' "${sent%% *}"
check 'sends the reply and the session' \
    '{"reply": "Let us switch to a graph database.", "session": "claude-code:abc-1"}' "${sent#* }"

# Without last_assistant_message: the text after the last typed prompt in
# the transcript, tool results and earlier turns left out.
cat >"$tmpbase/t.jsonl" <<'JSONL'
{"type":"user","message":{"role":"user","content":"first prompt"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"old answer"}]}}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"second prompt"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Switch to a graph database."}]}}
JSONL
run "{\"session_id\":\"abc-1\",\"transcript_path\":\"$tmpbase/t.jsonl\"}" >/dev/null
sent=$(tail -n 1 "$tmpbase/requests")
check 'reads the latest turn from the transcript' \
    '{"reply": "Looking.\n\nSwitch to a graph database.", "session": "claude-code:abc-1"}' "${sent#* }"

# Nothing to remind: silent.
printf '{"reminders":[],"remaining_today":5}' >"$tmpbase/response.json"
out=$(run '{"session_id":"abc-1","last_assistant_message":"Renamed the helper."}')
check 'no reminders prints nothing' "" "$out"

# A second pass of the same stop (stop_hook_active) is not checked again.
before=$(wc -l <"$tmpbase/requests")
out=$(run '{"session_id":"abc-1","last_assistant_message":"x y z","stop_hook_active":true}')
check 'stop_hook_active prints nothing' "" "$out"
check 'stop_hook_active sends nothing' "$before" "$(wc -l <"$tmpbase/requests")"

# A daemon slower than one second is skipped.
printf '{"reminders":[{"text":"late"}],"remaining_today":4}' >"$tmpbase/response.json"
touch "$tmpbase/slow"
start=$(date +%s)
out=$(run '{"last_assistant_message":"Switch to a graph database."}')
rc=$?
elapsed=$(( $(date +%s) - start ))
rm -f "$tmpbase/slow"
check 'slow daemon prints nothing' "" "$out"
check 'slow daemon exits 0' "0" "$rc"
[ "$elapsed" -le 3 ] && fast=yes || fast=no
check 'slow daemon does not hold the stop' yes "$fast"

# No daemon at all: silent, exit 0.
for case_name in 'daemon down' 'empty stdin' 'not json' 'empty reply'; do
    case "$case_name" in
        'daemon down') payload='{"last_assistant_message":"Switch to a graph database."}'; host='http://127.0.0.1:9' ;;
        'empty stdin') payload=''; host="$HOST" ;;
        'not json') payload='garbage'; host="$HOST" ;;
        'empty reply') payload='{"last_assistant_message":"   "}'; host="$HOST" ;;
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
