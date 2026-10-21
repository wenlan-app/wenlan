#!/usr/bin/env bash
# Bash tests for session-context.sh: the hook JSON on stdin drives the output
# and the $CLAUDE_ENV_FILE export. It must never fail a session.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/session-context.sh"
tmpbase="$(mktemp -d)"
trap 'rm -rf "$tmpbase"' EXIT

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

ID='00893aaf-19fa-41d2-8238-13269b9b3ca0'
LINE="Wenlan session: claude-code:$ID. Pass it as session when you call capture, write_page or update_brief."

envfile="$tmpbase/env"
out=$(printf '{"session_id":"%s","transcript_path":"/x/y.jsonl","cwd":"/work","hook_event_name":"SessionStart"}' "$ID" |
    CLAUDE_ENV_FILE="$envfile" bash "$HOOK")
check 'prints the session context line' "$LINE" "$out"
check 'exports WENLAN_SESSION to the env file' "export WENLAN_SESSION=claude-code:$ID" "$(cat "$envfile")"

# A second session start appends instead of clobbering the file.
printf '{"session_id":"second-1"}' | CLAUDE_ENV_FILE="$envfile" bash "$HOOK" >/dev/null
check 'appends to an existing env file' "2" "$(grep -c '^export WENLAN_SESSION=' "$envfile")"

# Pretty-printed JSON, session_id not first.
out=$(printf '{\n  "cwd": "/work",\n  "session_id": "%s"\n}\n' "$ID" | env -u CLAUDE_ENV_FILE bash "$HOOK")
check 'parses pretty-printed JSON' "$LINE" "$out"

# No env file: still prints the line, writes nothing.
out=$(printf '{"session_id":"%s"}' "$ID" | env -u CLAUDE_ENV_FILE bash "$HOOK")
check 'works without CLAUDE_ENV_FILE' "$LINE" "$out"

# An unwritable env file never fails the hook or hides the line.
out=$(printf '{"session_id":"%s"}' "$ID" | CLAUDE_ENV_FILE="$tmpbase/missing/dir/env" bash "$HOOK")
rc=$?
check 'unwritable env file still prints the line' "$LINE" "$out"
check 'unwritable env file exits 0' "0" "$rc"

# Silent, exit 0, nothing written for unusable input.
for case_name in 'empty stdin' 'no session_id' 'empty id' 'unsafe id' 'overlong id' 'not json'; do
    case "$case_name" in
        'empty stdin') payload='' ;;
        'no session_id') payload='{"cwd":"/work"}' ;;
        'empty id') payload='{"session_id":""}' ;;
        'unsafe id') payload='{"session_id":"a b;rm -rf /"}' ;;
        'overlong id') payload="{\"session_id\":\"$(printf 'a%.0s' $(seq 1 129))\"}" ;;
        'not json') payload='garbage' ;;
    esac
    rm -f "$envfile"
    out=$(printf '%s' "$payload" | CLAUDE_ENV_FILE="$envfile" bash "$HOOK")
    rc=$?
    check "$case_name prints nothing" "" "$out"
    check "$case_name exits 0" "0" "$rc"
    if [ -e "$envfile" ]; then written=present; else written=absent; fi
    check "$case_name writes no env file" absent "$written"
done

# A TTY or closed stdin must not hang the hook.
out=$(bash "$HOOK" </dev/null)
check 'closed stdin prints nothing' "" "$out"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
