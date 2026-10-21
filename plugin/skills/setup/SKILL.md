---
name: setup
description: >
  Frictionless setup. Detects a missing local runtime, installs or repairs it,
  configures local memory, and verifies the full plugin -> MCP -> local runtime
  round-trip. Run after `/plugin install wenlan@7xuanlu-wenlan`, or any time the user
  says "set up wenlan", "is wenlan working", "fix wenlan".
allowed-tools: ["Bash", "mcp__plugin_wenlan_wenlan__brief"]
user-invocable: false
---

# Setup

Reached through `/wenlan`: bare `/wenlan` starts this when Wenlan is not
ready, and so does asking to set up or fix it.

Self-healing setup. Goal: two user actions max: install plugin, type `/wenlan`.
Default backend is local memory: no local model, no API key, no prompts.

## Steps

Run in order. Stop and report at the first failure that needs human attention.
Otherwise, push through automatically.

### 1. Health probe

```bash
for i in 1 2 3; do
  curl -fsS -m 3 http://127.0.0.1:7878/api/health && break
  sleep 1
done
```

- 200 OK: continue to version drift probe.
- Anything else: continue to bootstrap.

### 2. Version drift probe

Compare daemon version vs plugin manifest version:

```bash
PLUGIN_JSON="${CLAUDE_PLUGIN_ROOT}/.claude-plugin/plugin.json"
"${CLAUDE_PLUGIN_ROOT}/scripts/version-drift.sh" --health-url http://127.0.0.1:7878/api/health --manifest "$PLUGIN_JSON"
```

It prints one line, `daemon=<v> expected=<v> release=<v> result=<result>`,
comparing numerically per component with build metadata (`+g<sha>`) stripped.
`older` means the daemon is older than the plugin.

- `result=same`: continue to doctor.
- If the probe cannot run because the runtime is down: continue to bootstrap.
- `result=unknown` with a healthy runtime (manifest unreadable or no version
  reported): continue to doctor; the next `/wenlan` will report the mismatch
  again if one exists.
- If mismatch, check the direction before repairing. `result=newer`: the
  daemon release is newer than the plugin release, so the plugin cache is
  stale, not the runtime. Skip the repair below (it would only reinstall the
  same-or-latest runtime and restart a healthy daemon) and go straight to the
  stop message in step 4 — update the plugin, restart, rerun.
- Only if the daemon release is older than the plugin release, repair the
  runtime (`result=older`):

```bash
npx -y wenlan@0.19.3 setup
export PATH="$HOME/.wenlan/bin:$PATH"
```

That is the same pinned package step 3 uses; the same platform limits and
manual fallback apply. Then continue to the health and version re-probe below.
The package deliberately installs the latest stable runtime. Do not downgrade a
newer runtime to match a stale plugin cache.

### 3. Bootstrap

Detect whether the `wenlan` CLI is on PATH:

```bash
command -v wenlan >/dev/null 2>&1 && echo present || echo absent
```

If absent, install and configure local memory with the `wenlan` npm package,
pinned to this plugin's exact version. It installs the runtime into
`~/.wenlan/bin`, runs `wenlan setup --basic` and `wenlan background on`, and
prints the status:

```bash
npx -y wenlan@0.19.3 setup
export PATH="$HOME/.wenlan/bin:$PATH"
```

The package supports macOS Apple Silicon and needs Node.js. On any other
platform, or when `npx` is missing or the command fails, do not fetch or run an
installer yourself. Stop and tell the user to install the runtime by following
<https://github.com/wenlan-app/wenlan/blob/main/docs/setup-with-ai.md#install-the-runtime>,
then run `/wenlan` again.

If present but the local runtime is down:

```bash
wenlan setup --basic 2>/dev/null || true
wenlan background on
```

`wenlan setup --basic` is idempotent. `wenlan background on` starts the
managed background process.

### 4. Re-probe health and version

If `wenlan background on` exited non-zero, print its output and stop: it
already waited for the daemon and named what went wrong. Otherwise poll for up
to 240 seconds, the budget the first-run checks use, because a first boot
downloads the embedding model (about 210 MB) before it answers:

```bash
healthy=
deadline=$((SECONDS + 240))
while [ "$SECONDS" -lt "$deadline" ]; do
  if curl -fsS -m 3 http://127.0.0.1:7878/api/health >/dev/null 2>&1; then
    healthy=1
    break
  fi
  sleep 1
done
if [ -z "$healthy" ]; then
  W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
  "$W" doctor
  exit 1
fi
```

When the loop ends without a healthy answer, the doctor output names the daemon
state and where its logs are (a file path on macOS, `journalctl` on Linux);
stop there. Likely causes: the model download is still running or failed,
launchd load failure, port 7878 already in use, or a local runtime crash.

Once healthy, rerun the step 2 command. If `result` is still `older` or
`newer`, stop instead of claiming setup succeeded:

```text
Runtime and plugin versions still differ after repair; update the Wenlan plugin,
restart Claude Code, then run /wenlan again.
```

This usually means the runtime is newer than the plugin cached by the current
Claude Code process. Updating the plugin is safer than silently downgrading the
runtime, and the restart is required before this session can load new plugin
code.

### 5. Doctor

Run the Wenlan CLI doctor through the resolved binary path:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" doctor
```

Expected: local memory configured. Capture the mode string for the final report.

### 6. MCP round-trip

Call the Wenlan MCP `brief` tool as a read-only round-trip.

```text
brief()
```

Pass: continue. Fail: MCP is not wired. Tell the user:
"wenlan-mcp did not respond. Restart Claude Code so the plugin's `.mcp.json`
respawns the server."

### 7. Ready report

Print:

```text
Wenlan ready.
  Runtime:  up on 127.0.0.1:7878
  Mode:     <mode from CLI doctor>
  MCP:      connected
  Data:     ~/.wenlan/  (pages, sessions; database location in docs/setup-and-data.md)
  Try:      /wenlan <what you want>, /capture <thing>, /recall <query>
```

End here. Do not invoke `/wenlan` or any other skill after the report; the
`Try:` line is the whole orientation.

## Optional upgrades

Steep, the optional model-backed background upkeep, is off by default; a connected AI tool writes pages without it. To turn it on:

Run `wenlan steep status`, help the user choose the exact Everyday and Synthesis
sources, then run `wenlan steep configure --everyday <source> --synthesis
<source>`. The CLI itself shows the task mapping and cloud/on-device disclosure
and obtains the one confirmation. Never add `--yes` unless the user already
stated that exact mapping. `wenlan steep disable` reverses the consent
without deleting the model or key.

## When to use

- Right after `/plugin install wenlan@7xuanlu-wenlan`.
- The plugin reported "Wenlan is not running" or a version mismatch and named
  `/wenlan` as the fix.
- User says "set up wenlan", "is it working", "reinstall wenlan".

## When NOT to use

- Runtime already verified this session: `/brief` instead.
- Editing one config field: rerun the CLI doctor command from Step 5 or edit the
  settings file directly.
