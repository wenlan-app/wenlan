# Set up Wenlan with your AI client

Use this guide when an AI assistant is setting up Wenlan on the user's behalf. Configure only the client in the current conversation unless the user explicitly asks for additional clients.

## Target outcome

Setup is complete only when:

1. The Wenlan runtime is healthy.
2. The current AI client has the Wenlan plugin, an MCP connection, or a working `wenlan` CLI on the machine where the runtime runs.
3. A small capture can be recalled from the same client.

Do not report success after editing configuration alone. A live round trip is the proof.

On the Claude Code plugin path, `/wenlan` (which runs setup when Wenlan is not ready) already proves the round trip with a read-only `brief` call, which satisfies step 3.

## Install the runtime

Detect the host before choosing the runtime install path:

| Host | Install path |
|---|---|
| macOS Apple Silicon | Run `npx -y wenlan setup` (requires Node.js). Without Node.js, run `curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh \| bash` then `wenlan setup --basic`. |
| Linux x64 or ARM64 | Run the shell installer below. |
| Windows x64 | Download `wenlan-windows-x64.zip` from [Releases](https://github.com/wenlan-app/wenlan/releases/latest). |
| macOS Intel | No supported complete-runtime install; see the [platform note](../crates/wenlan-cli/README.md#macos-intel). |

On Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash
export PATH="$HOME/.wenlan/bin:$PATH"
wenlan setup --basic
wenlan background on
wenlan status
```

On Windows, extract the archive as a unit into a user-owned directory on `PATH`. Keep `onnxruntime.dll` beside `wenlan.exe`, `wenlan-server.exe`, and `wenlan-mcp.exe`, then run:

```bash
wenlan setup --basic
wenlan background on
wenlan status
```

## Claude Code

For Claude Code or Claude Desktop, use the plugin instructions below or [connect through MCP](#mcp-only-setup-and-other-clients).

Install the Wenlan plugin from the Claude directory (claude.ai, Cowork, or Claude Code) or from its public marketplace:

```bash
claude plugin marketplace add wenlan-app/wenlan
claude plugin install wenlan@7xuanlu-wenlan
```

Start a new Claude Code session if requested, then run `/wenlan`. When Wenlan is not ready it runs setup, which installs or repairs the local runtime and verifies the MCP round trip. Detailed workflows: [Claude Code plugin](../plugin/README.md).

## Codex

Install the runtime using the host-specific path above, then install the plugin:

```bash
codex plugin marketplace add wenlan-app/wenlan
codex plugin add wenlan@7xuanlu-wenlan
```

Start a new Codex task so the plugin and MCP server load, then type `$wenlan` (or the exact form `$wenlan:wenlan`). Detailed workflows: [Codex plugin](../plugin-codex/README.md).

## Grok Bot

Grok Bot uses the Wenlan library on the user's computer in two ways. Both run there, through the commands Grok Bot is allowed to run on that computer (on a Mac, the machine it can shell into). A command on Grok Bot's own Linux computer talks to a daemon on that computer, not to the library on the user's Mac.

There is no `wenlan connect grok-bot` target, and **Add a tool** does not set Grok Bot up. Grok Bot does not keep a local MCP config file. Its MCP path is a short session per command, not a persistent registered connector.

### CLI on the user's computer

Install the runtime with the host-specific path above, on the computer that holds the library. Pass `--agent-name grok-bot` on each command, or export `WENLAN_AGENT_NAME=grok-bot` for the shell. Without a name, the daemon records the caller as `unknown`.

```bash
wenlan --agent-name grok-bot recall "what we decided"
wenlan --agent-name grok-bot memories
wenlan --agent-name grok-bot spaces list
wenlan --agent-name grok-bot capture "the decision, in one sentence"
```

`recall` searches memories and the pages that match. `memories` lists recent memories. `spaces list` lists Spaces. `capture` stores one memory. In an interactive terminal it prints `Stored memory <id>`; when stdout is piped, it returns JSON with the id in `source_id`.

### MCP over a local shell

Run this function in a shell on the computer that holds the library. It starts a short-lived MCP process for each call, waits for that call's reply, and then exits. The connector calls `http://127.0.0.1:7878` on that same computer. Python 3 is required.

```bash
wenlan_mcp_call() {
  python3 - "$@" <<'PY'
import json, os, queue, subprocess, sys, threading, time

RESPONSE_TIMEOUT = 60
tool, arguments = sys.argv[1], json.loads(sys.argv[2])
binary = os.path.expanduser("~/.wenlan/bin/wenlan-mcp")
proc = subprocess.Popen(
    [binary, "--agent-name", "grok-bot"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=None,
)
chunks = queue.Queue()

def pump_stdout():
    while True:
        try:
            chunk = os.read(proc.stdout.fileno(), 65536)
        except OSError:
            chunk = b""
        chunks.put(chunk)
        if not chunk:
            return

threading.Thread(target=pump_stdout, daemon=True).start()
buffer = b""

def send(message):
    proc.stdin.write((json.dumps(message) + "\n").encode())
    proc.stdin.flush()

def receive(request_id):
    global buffer
    deadline = time.monotonic() + RESPONSE_TIMEOUT
    while True:
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            if not line:
                continue
            message = json.loads(line)
            if message.get("id") == request_id:
                if "error" in message:
                    raise RuntimeError("JSON-RPC error: " + json.dumps(message["error"]))
                result = message.get("result")
                if not isinstance(result, dict) or result.get("isError"):
                    raise RuntimeError("tool call failed: " + json.dumps(result))
                return result
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("timed out waiting for MCP response")
        try:
            chunk = chunks.get(timeout=remaining)
        except queue.Empty:
            raise RuntimeError("timed out waiting for MCP response")
        if not chunk:
            raise RuntimeError("wenlan-mcp closed stdout before replying")
        buffer += chunk

try:
    send({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
        "protocolVersion":"2025-11-25", "capabilities":{},
        "clientInfo":{"name":"grok-bot", "version":"1"}}})
    receive(1)
    send({"jsonrpc":"2.0", "method":"notifications/initialized"})
    send({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{
        "name":tool, "arguments":arguments}})
    print(json.dumps(receive(2), ensure_ascii=False))
    proc.stdin.close()
    proc.wait(timeout=5)
finally:
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    proc.stdout.close()
PY
}

wenlan_mcp_call recall '{"query":"what we decided"}'
```

The function waits up to 60 seconds for each response, prints the tool result as JSON, and reports a JSON-RPC or tool error as a failed command. Server logs go to stderr.

### Verify

On the computer where the daemon runs:

```bash
wenlan status
wenlan --agent-name grok-bot capture "Wenlan Grok Bot setup check. Disposable."
wenlan --agent-name grok-bot recall "Wenlan Grok Bot setup check"
```

The round trip is proven when `wenlan status` reports a healthy runtime and `recall` returns the captured sentence. Use the id from `Stored memory <id>` in an interactive terminal, or the `source_id` field when the CLI returns JSON (for example, through a bot shell). After defining `wenlan_mcp_call` above in the same shell, delete that memory with `forget`:

```bash
wenlan_mcp_call forget '{"memory_id":"<id>"}'
```

Replace `<id>` with the id `capture` printed. The `wenlan` CLI has no forget command.

## MCP-only setup and other clients

Install the runtime using the host-specific path above, then configure only the client the user named:

```bash
wenlan connect <client>
```

Use `claude-code` or `codex` when the user wants MCP without the plugin. Supported values are `claude-code`, `codex`, `cursor`, `claude-desktop`, `vscode`, and `gemini`. For Grok Bot, follow [Grok Bot](#grok-bot). The CLI makes a backup before replacing an existing JSON configuration. Full command reference: [CLI and MCP setup](../crates/wenlan-cli/README.md).

The desktop App connects more tools than the CLI: **Add a tool** in its Settings also sets up GitHub Copilot, Antigravity, Windsurf, Kiro, OpenCode, and Muse Code, and copies Wenlan's skills to the tools that read them. If the user has the App, point them there for those tools instead of editing their configuration by hand.

## Verify

First confirm the local runtime:

```bash
wenlan status
```

Then use the current client's Wenlan tools to capture a disposable sentence and recall it. Delete the test memory afterward if the client exposes `forget`. If the tools are not visible, start a new client session and verify again before declaring setup complete. For Grok Bot, run the round trip in [Grok Bot](#grok-bot), including MCP `forget`.
