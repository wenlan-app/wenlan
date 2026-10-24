# Setup

Reached through `/wenlan`: bare `/wenlan` starts this when Wenlan is not
ready, and so does asking to set up or fix it.

Self-healing setup. Goal: the user only types `/wenlan`.
Default backend is local memory: no local model, no API key, no prompts.

## Steps

Run in order. Stop and report at the first failure that needs human attention.
Otherwise, push through automatically.

### 1. Health probe

```bash
for i in 1 2 3; do
  curl -fsS -m 3 "${WENLAN_HOST:-http://127.0.0.1:7878}/api/health" && break
  sleep 1
done
```

- 200 OK: continue to doctor.
- Anything else: continue to bootstrap.

### 2. Bootstrap

Detect whether the `wenlan` CLI is on PATH:

```bash
{ command -v wenlan >/dev/null 2>&1 || [ -x "$HOME/.wenlan/bin/wenlan" ]; } && echo present || echo absent
```

If absent, the runtime is not installed. Do not fetch or run an installer
yourself. Stop and tell the user to install the runtime by following
<https://github.com/wenlan-app/wenlan/blob/main/docs/setup-with-ai.md#install-the-runtime>,
then run `/wenlan` again.

If present but the local runtime is down:

```bash
export PATH="$HOME/.wenlan/bin:$PATH"
wenlan setup --basic 2>/dev/null || true
wenlan background on
```

`wenlan setup --basic` is idempotent. `wenlan background on` starts the
managed background process.

### 3. Re-probe health

If `wenlan background on` exited non-zero, print its output and stop: it
already waited for the daemon and named what went wrong. Otherwise poll for up
to 240 seconds, the budget the first-run checks use, because a first boot
downloads the embedding model (about 210 MB) before it answers:

```bash
healthy=
deadline=$((SECONDS + 240))
while [ "$SECONDS" -lt "$deadline" ]; do
  if curl -fsS -m 3 "${WENLAN_HOST:-http://127.0.0.1:7878}/api/health" >/dev/null 2>&1; then
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

### 4. Doctor

Run the Wenlan CLI doctor through the resolved binary path:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
"$W" doctor
```

Expected: local memory configured. Capture the mode string for the final report.

### 5. MCP round-trip

Call the Wenlan MCP `brief` tool as a read-only round-trip.

```text
brief()
```

Pass: continue. Fail: MCP is not wired. Tell the user:
"The Wenlan MCP server did not respond. Check that the `wenlan` server is listed
and enabled in this tool's MCP settings (the Wenlan app's Set up button adds it),
then restart this tool so it starts the server."

### 6. Ready report

Print:

```text
Wenlan ready.
  Runtime:  up on 127.0.0.1:7878
  Mode:     <mode from CLI doctor>
  MCP:      connected
  Data:     ~/.wenlan/  (pages, sessions; database location in docs/setup-and-data.md)
  Try:      /wenlan <what you want>, /wenlan-capture <thing>, /wenlan-recall <query>
```

End here. Do not invoke `/wenlan` or any other skill after the report; the
`Try:` line is the whole orientation.

## Optional upgrades

Steep keeps pages in step with their sources. It runs in two modes: On demand, where a connected AI tool does it with `/wenlan steep`, and Automatic, where Wenlan's own model does it in the background. Automatic needs a model and is not set up by default. To set it up:

Run `wenlan steep status`, help the user choose the exact Everyday and Synthesis
sources, then run `wenlan steep configure --everyday <source> --synthesis
<source>`. The CLI itself shows the task mapping and cloud/on-device disclosure
and obtains the one confirmation. Never add `--yes` unless the user already
stated that exact mapping. `wenlan steep disable` reverses the consent
without deleting the model or key.

## When to use

- Right after the Wenlan app set this tool up.
- A Wenlan skill reported "Wenlan is not running" and named `/wenlan` as the
  fix.
- User says "set up wenlan", "is it working", "reinstall wenlan".

## When NOT to use

- Runtime already verified this session: `/wenlan-brief` instead.
- Editing one config field: rerun the CLI doctor command from Step 4 or edit the
  settings file directly.
