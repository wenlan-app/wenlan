#!/usr/bin/env bash
# Stop: once per Claude Code or Codex session, ask the AI to hand off when the
# latest transcript usage reaches 80 percent. Only the last 2 MiB is read.
set -u
command -v python3 >/dev/null 2>&1 || exit 0

input=""
if [ ! -t 0 ]; then
  IFS= read -r -t 2 -d '' input 2>/dev/null || true
fi
[ -n "$input" ] || exit 0

state_root=""
if [ -n "${WENLAN_DATA_DIR:-}" ]; then
  state_root="$WENLAN_DATA_DIR/hook-state/handoff-reminders"
else
  state_root="${TMPDIR:-/tmp}/wenlan-hook-state/handoff-reminders"
fi

printf '%s' "$input" | WENLAN_HANDOFF_STATE="$state_root" python3 -I -c '
import json, os, sys
try:
    hook = json.load(sys.stdin)
except Exception:
    raise SystemExit(0)
if hook.get("stop_hook_active"):
    raise SystemExit(0)
sid = str(hook.get("session_id") or "")
if not sid or len(sid) > 128 or any(not (c.isascii() and (c.isalnum() or c in "._-")) for c in sid):
    raise SystemExit(0)
def direct_percentage(value):
    if isinstance(value, dict):
        direct = value.get("used_percentage")
        if isinstance(direct, (int, float)):
            return float(direct)
        for child in value.values():
            found = direct_percentage(child)
            if found is not None: return found
    elif isinstance(value, list):
        for child in value:
            found = direct_percentage(child)
            if found is not None: return found
    elif isinstance(value, str) and "used_percentage" in value:
        try: return direct_percentage(json.loads(value))
        except Exception: pass
    return None

def tail_json(path, limit=2 * 1024 * 1024):
    try:
        with open(path, "rb") as source:
            source.seek(0, os.SEEK_END)
            size = source.tell()
            start = max(0, size - limit)
            source.seek(start)
            data = source.read(limit)
    except Exception:
        return []
    if start:
        newline = data.find(b"\n")
        data = b"" if newline < 0 else data[newline + 1:]
    entries = []
    for raw in data.decode("utf-8", "replace").splitlines():
        try: entries.append(json.loads(raw))
        except Exception: pass
    return entries

def number(value):
    try:
        return float(value) if isinstance(value, (int, float, str)) else 0.0
    except (TypeError, ValueError):
        return 0.0

def positive_integer(value):
    text = str(value or "").strip()
    if not text.isdigit():
        return None
    parsed = int(text)
    return parsed if parsed > 0 else None

def configured_model(hook):
    for name in ("ANTHROPIC_MODEL", "CLAUDE_MODEL"):
        model = str(os.environ.get(name) or "").strip()
        if model:
            return model

    cwd = hook.get("cwd")
    paths = []
    if isinstance(cwd, str) and cwd:
        paths.extend((
            os.path.join(cwd, ".claude", "settings.local.json"),
            os.path.join(cwd, ".claude", "settings.json"),
        ))
    config_dir = str(os.environ.get("CLAUDE_CONFIG_DIR") or "").strip()
    if config_dir:
        paths.append(os.path.join(config_dir, "settings.json"))
    else:
        paths.append(os.path.join(os.path.expanduser("~"), ".claude", "settings.json"))
    for path in paths:
        try:
            with open(path, encoding="utf-8") as source:
                settings = json.load(source)
        except Exception:
            continue
        if not isinstance(settings, dict):
            continue
        model = settings.get("model")
        if isinstance(model, str) and model.strip():
            return model.strip()
    return ""

def claude_context_window(hook, used_tokens):
    for name in ("WENLAN_CONTEXT_WINDOW", "CLAUDE_CODE_AUTO_COMPACT_WINDOW"):
        window = positive_integer(os.environ.get(name))
        if window is not None:
            return window
    if configured_model(hook).endswith("[1m]"):
        return 1_000_000
    if used_tokens > 200_000:
        return 1_000_000
    return 200_000

def transcript_percentage(hook):
    path = hook.get("transcript_path")
    if not isinstance(path, str) or not path:
        return None
    entries = tail_json(path)
    if "turn_id" in hook:  # Codex rollout JSONL
        for entry in reversed(entries):
            event = entry
            payload = entry.get("payload") if isinstance(entry, dict) else None
            if isinstance(payload, dict) and payload.get("type") == "token_count":
                event = payload
            if not isinstance(event, dict) or event.get("type") != "token_count":
                continue
            info = event.get("info")
            if not isinstance(info, dict):
                continue
            last = info.get("last_token_usage")
            window = number(info.get("model_context_window"))
            used_tokens = number(last.get("input_tokens")) if isinstance(last, dict) else 0
            if window > 0 and used_tokens >= 0:
                return used_tokens * 100.0 / window
        return None
    for entry in reversed(entries):  # Claude Code transcript JSONL
        if not isinstance(entry, dict) or entry.get("type") != "assistant":
            continue
        message = entry.get("message")
        usage = message.get("usage") if isinstance(message, dict) else None
        if not isinstance(usage, dict):
            return None
        used_tokens = sum(number(usage.get(key)) for key in (
            "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
        if used_tokens <= 0:
            return None
        window = claude_context_window(hook, used_tokens)
        return used_tokens * 100.0 / window
    return None

used = transcript_percentage(hook)
if used is None:
    # Future hook versions may supply the statusline value directly.
    used = direct_percentage(hook)
if used is None or used < 80:
    raise SystemExit(0)
root = os.environ["WENLAN_HANDOFF_STATE"]
try:
    os.makedirs(root, mode=0o700, exist_ok=True)
    path = os.path.join(root, sid)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.write(fd, str(used).encode())
    os.close(fd)
except FileExistsError:
    raise SystemExit(0)
except Exception:
    raise SystemExit(0)
print(json.dumps({
    "decision": "block",
    "reason": "Context use reached 80%. Run the Wenlan handoff tool now, then finish this turn."
}))
' 2>/dev/null
exit 0
