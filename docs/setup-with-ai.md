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

`recall` searches memories and the pages that match. `memories` lists recent memories. `spaces list` lists Spaces. `capture` stores one memory and prints its id (`Stored memory <id>`).

### MCP over a local shell

On the user's computer, start one `wenlan-mcp` process, pipe it newline-delimited JSON-RPC, read the replies from stdout, and let the process exit. Each invocation is its own session. The connector calls `http://127.0.0.1:7878` on that same computer.

```bash
~/.wenlan/bin/wenlan-mcp --agent-name grok-bot <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"grok-bot","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"recall","arguments":{"query":"what we decided"}}}
EOF
```

Send `initialize`, then `notifications/initialized`, then the call. Replies are one JSON object per line. `initialize` returns the server info. `tools/call` with `name` `recall` searches the library. The same session shape with `method` `tools/list` returns the local tools, including `recall`, `brief`, `get_page_sources`, `capture`, `write_page`, and `forget`. Logs go to stderr.

### Verify

On the computer where the daemon runs:

```bash
wenlan status
wenlan --agent-name grok-bot capture "Wenlan Grok Bot setup check. Disposable."
wenlan --agent-name grok-bot recall "Wenlan Grok Bot setup check"
```

The round trip is proven when `wenlan status` reports a healthy runtime and `recall` returns the captured sentence. `capture` prints `Stored memory <id>`. Delete that memory in a short MCP session with `forget`:

```bash
~/.wenlan/bin/wenlan-mcp --agent-name grok-bot <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"grok-bot","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"forget","arguments":{"memory_id":"<id>"}}}
EOF
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
