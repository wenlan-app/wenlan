#!/usr/bin/env bash
set -eu
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/handoff-reminder.sh"
tmpbase="$(mktemp -d)"
trap 'rm -rf "$tmpbase"' EXIT
mkdir -p "$tmpbase/home" "$tmpbase/tmp" "$tmpbase/empty-project"

# Every invocation starts with the relevant host variables scrubbed and a
# scratch HOME, so this test can never inspect the developer's Claude config.
run() {
    input="$1"
    shift
    printf '%s' "$input" | env \
        -u WENLAN_CONTEXT_WINDOW \
        -u CLAUDE_CODE_AUTO_COMPACT_WINDOW \
        -u ANTHROPIC_MODEL \
        -u CLAUDE_MODEL \
        -u CLAUDE_CONFIG_DIR \
        HOME="$tmpbase/home" \
        WENLAN_DATA_DIR="$tmpbase/data" \
        TMPDIR="$tmpbase/tmp" \
        "$@" bash "$HOOK"
}

cat >"$tmpbase/claude-170k.jsonl" <<'JSONL'
{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":2,"cache_creation_input_tokens":377,"cache_read_input_tokens":169621,"output_tokens":464}}}
JSONL
cat >"$tmpbase/claude-209k.jsonl" <<'JSONL'
{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":2,"cache_creation_input_tokens":377,"cache_read_input_tokens":209247,"output_tokens":464}}}
JSONL
cat >"$tmpbase/codex-at.jsonl" <<'JSONL'
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":900000},"last_token_usage":{"input_tokens":800000},"model_context_window":1000000}}}
JSONL

stop_170() {
    printf '{"session_id":"%s","transcript_path":"%s","cwd":"%s","hook_event_name":"Stop","stop_hook_active":false}' \
        "$1" "$tmpbase/claude-170k.jsonl" "$2"
}

# 1. WENLAN_CONTEXT_WINDOW wins over the auto-compact window.
explicit=$(run "$(stop_170 s-explicit "$tmpbase/empty-project")" \
    WENLAN_CONTEXT_WINDOW=200000 CLAUDE_CODE_AUTO_COMPACT_WINDOW=1000000)
printf '%s' "$explicit" | grep -Fq 'handoff'

# 2. Claude Code's positive auto-compact window is the next source. A zero
# Wenlan override is ignored rather than masking it.
auto=$(run "$(stop_170 s-auto "$tmpbase/empty-project")" \
    WENLAN_CONTEXT_WINDOW=0 CLAUDE_CODE_AUTO_COMPACT_WINDOW=1000000)
[ -z "$auto" ]

# 3a. Environment model configuration can select the 1M window.
anthropic_model=$(run "$(stop_170 s-anthropic-model "$tmpbase/empty-project")" \
    ANTHROPIC_MODEL='claude-opus-5-5[1m]')
[ -z "$anthropic_model" ]
claude_model=$(run "$(stop_170 s-claude-model "$tmpbase/empty-project")" \
    CLAUDE_MODEL='claude-opus-5-5[1m]')
[ -z "$claude_model" ]
env_model_order=$(run "$(stop_170 s-env-model-order "$tmpbase/empty-project")" \
    ANTHROPIC_MODEL='claude-opus-5-5' CLAUDE_MODEL='claude-opus-5-5[1m]')
printf '%s' "$env_model_order" | grep -Fq 'handoff'

# 3b. Project-local settings are read first.
mkdir -p "$tmpbase/project-local/.claude"
printf '{"model":"claude-opus-5-5[1m]"}\n' >"$tmpbase/project-local/.claude/settings.local.json"
project_local=$(run "$(stop_170 s-project-local "$tmpbase/project-local")")
[ -z "$project_local" ]

# A valid higher-priority model wins even when the next settings file says 1M.
mkdir -p "$tmpbase/project-order/.claude"
printf '{"model":"claude-opus-5-5"}\n' >"$tmpbase/project-order/.claude/settings.local.json"
printf '{"model":"claude-opus-5-5[1m]"}\n' >"$tmpbase/project-order/.claude/settings.json"
project_order=$(run "$(stop_170 s-project-order "$tmpbase/project-order")")
printf '%s' "$project_order" | grep -Fq 'handoff'

# Parse errors are ignored, then the next settings file is considered.
mkdir -p "$tmpbase/project-fallback/.claude"
printf '{not json\n' >"$tmpbase/project-fallback/.claude/settings.local.json"
printf '{"model":"claude-opus-5-5[1m]"}\n' >"$tmpbase/project-fallback/.claude/settings.json"
project_fallback=$(run "$(stop_170 s-project-fallback "$tmpbase/project-fallback")")
[ -z "$project_fallback" ]

# The configured Claude directory is used before the scratch HOME fallback.
mkdir -p "$tmpbase/claude-config"
printf '{"model":"claude-opus-5-5[1m]"}\n' >"$tmpbase/claude-config/settings.json"
configured_global=$(run "$(stop_170 s-configured-global "$tmpbase/empty-project")" \
    CLAUDE_CONFIG_DIR="$tmpbase/claude-config")
[ -z "$configured_global" ]

# Without CLAUDE_CONFIG_DIR, the same lookup is confined to the scratch HOME.
mkdir -p "$tmpbase/home-config/.claude"
printf '{"model":"claude-opus-5-5[1m]"}\n' >"$tmpbase/home-config/.claude/settings.json"
home_global=$(run "$(stop_170 s-home-global "$tmpbase/empty-project")" \
    HOME="$tmpbase/home-config")
[ -z "$home_global" ]

# 4. With no configuration, tokens already over 200k imply a 1M window. This
# real 1M-shaped record must not fire at 209,626 used tokens.
over_200k=$(run "$(printf '{"session_id":"s-over-200k","transcript_path":"%s","cwd":"%s","hook_event_name":"Stop","stop_hook_active":false}' "$tmpbase/claude-209k.jsonl" "$tmpbase/empty-project")")
[ -z "$over_200k" ]

# 5. Otherwise the window is 200k, so 170k fires and only fires once.
default_first=$(run "$(stop_170 s-default "$tmpbase/empty-project")")
printf '%s' "$default_first" | grep -Fq '"decision": "block"'
printf '%s' "$default_first" | grep -Fq 'handoff'
default_again=$(run "$(stop_170 s-default "$tmpbase/empty-project")")
[ -z "$default_again" ]

# The Codex transcript path and future direct percentage fallback remain intact.
codex=$(run "$(printf '{"session_id":"s-codex","transcript_path":"%s","cwd":"%s","model":"gpt-6.2","turn_id":"t1","hook_event_name":"Stop","stop_hook_active":false}' "$tmpbase/codex-at.jsonl" "$tmpbase/empty-project")")
printf '%s' "$codex" | grep -Fq 'handoff'
missing=$(run "$(printf '{"session_id":"s-missing","transcript_path":"%s","cwd":"%s","hook_event_name":"Stop","stop_hook_active":false}' "$tmpbase/missing.jsonl" "$tmpbase/empty-project")")
[ -z "$missing" ]
unreadable=$(run "$(printf '{"session_id":"s-unreadable","transcript_path":"%s","cwd":"%s","hook_event_name":"Stop","stop_hook_active":false}' "$tmpbase" "$tmpbase/empty-project")")
[ -z "$unreadable" ]
future=$(run '{"session_id":"s-future","used_percentage":81,"hook_event_name":"Stop","stop_hook_active":false}')
printf '%s' "$future" | grep -Fq 'handoff'

printf '17 passed, 0 failed\n'
