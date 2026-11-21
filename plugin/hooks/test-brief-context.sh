#!/usr/bin/env bash
set -eu
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/brief-context.sh"
tmpbase="$(mktemp -d)"
server_pid=""
cleanup() {
    if [ -n "$server_pid" ]; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    rm -rf "$tmpbase"
}
trap cleanup EXIT

cat >"$tmpbase/server.py" <<'PY'
import http.server, json, os, sys
base = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        open(os.path.join(base, "request"), "wb").write(self.rfile.read(n))
        body = open(os.path.join(base, "response"), "rb").read()
        self.send_response(200); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *args): pass
s=http.server.ThreadingHTTPServer(("127.0.0.1",0),H)
open(os.path.join(base,"port"),"w").write(str(s.server_address[1])); s.serve_forever()
PY
cat >"$tmpbase/response" <<'JSON'
{"state":"ready","space":"work","brief":{"space_id":"s","space":"work","last_session_summary":"","version":3,"active":[{"id":"item-1","text":"Ship S2","details":"SECRET DETAIL","state":"active","added_at":1,"version":2}],"backlog":[{"id":"item-2","text":"Later work","state":"backlog","added_at":1,"version":1}]}}
JSON
python3 -I "$tmpbase/server.py" "$tmpbase" & server_pid=$!
for _ in $(seq 1 50); do [ -s "$tmpbase/port" ] && break; sleep 0.05; done
[ -s "$tmpbase/port" ]
host="http://127.0.0.1:$(cat "$tmpbase/port")"

mkdir -p "$tmpbase/bin"
cat >"$tmpbase/bin/wenlan" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "${WENLAN_NO_AUTOSTART:-unset}" >>"$WENLAN_STUB_RECORD"
printf '%s\n' "$*" >>"$WENLAN_STUB_ARGS"
exit 1
SH
chmod +x "$tmpbase/bin/wenlan"
env -u WENLAN_NO_AUTOSTART \
    PATH="$tmpbase/bin:$PATH" \
    CLAUDE_PLUGIN_ROOT="$SCRIPT_DIR/.." \
    WENLAN_STUB_RECORD="$tmpbase/wenlan-env" \
    WENLAN_STUB_ARGS="$tmpbase/wenlan-args" \
    WENLAN_HOST="$host" \
    bash "$HOOK" <<JSON >/dev/null
{"cwd":"$PWD"}
JSON
[ -s "$tmpbase/wenlan-env" ]
[ -z "$(grep -v '^1$' "$tmpbase/wenlan-env")" ]
! grep -q 'import-pending' "$tmpbase/wenlan-args"

env -u WENLAN_NO_AUTOSTART \
    PATH="$tmpbase/bin:$PATH" \
    WENLAN_SPACE=work \
    CLAUDE_PLUGIN_ROOT="$SCRIPT_DIR/.." \
    WENLAN_STUB_RECORD="$tmpbase/wenlan-env" \
    WENLAN_STUB_ARGS="$tmpbase/wenlan-args" \
    WENLAN_HOST="$host" \
    bash "$HOOK" <<JSON >/dev/null
{"cwd":"$PWD"}
JSON
[ -z "$(grep -v '^1$' "$tmpbase/wenlan-env")" ]
grep -Eq '^--space .+ handoff import-pending$' "$tmpbase/wenlan-args"

mkdir -p "$tmpbase/no-space-plugin/scripts"
cat >"$tmpbase/no-space-plugin/scripts/resolve-space.sh" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod +x "$tmpbase/no-space-plugin/scripts/resolve-space.sh"
: >"$tmpbase/wenlan-args"
no_space=$(printf '{"cwd":"%s"}' "$PWD" | env -u WENLAN_SPACE \
    PATH="$tmpbase/bin:$PATH" \
    CLAUDE_PLUGIN_ROOT="$tmpbase/no-space-plugin" \
    WENLAN_STUB_RECORD="$tmpbase/wenlan-env" \
    WENLAN_STUB_ARGS="$tmpbase/wenlan-args" \
    WENLAN_HOST="$host" \
    bash "$HOOK")
[ -z "$no_space" ]
! grep -q 'import-pending' "$tmpbase/wenlan-args"

out=$(printf '{"cwd":"%s"}' "$PWD" | WENLAN_HOST="$host" WENLAN_SPACE=work bash "$HOOK")
printf '%s' "$out" | grep -Fq 'item-1: Ship S2'
printf '%s' "$out" | grep -Fq 'item-2: Later work'
! printf '%s' "$out" | grep -Fq 'SECRET DETAIL'
[ "$(printf '%s' "$out" | wc -c | tr -d ' ')" -le 8000 ]
codex=$(printf '{"cwd":"%s","model":"gpt-6.2","source":"startup","hook_event_name":"SessionStart"}' "$PWD" | WENLAN_HOOK_HOST=codex WENLAN_HOST="$host" WENLAN_SPACE=work bash "$HOOK")
printf '%s' "$codex" | python3 -c 'import json,sys
body=json.load(sys.stdin)
assert body["hookSpecificOutput"]["hookEventName"] == "SessionStart"
assert "item-1: Ship S2" in body["hookSpecificOutput"]["additionalContext"]'
down=$(printf '{"cwd":"%s"}' "$PWD" | WENLAN_HOST=http://127.0.0.1:9 WENLAN_SPACE=work bash "$HOOK")
[ -z "$down" ]
printf '7 passed, 0 failed\n'
