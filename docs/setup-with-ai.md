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

Grok Bot reads and writes the user's Wenlan library by running the `wenlan` CLI on the user's computer. It can also use `wenlan-mcp` when that process starts on the same computer as the daemon.

There is no `wenlan connect grok-bot` target, and **Add a tool** does not set Grok Bot up. Grok Bot does not keep a local MCP config file this command can update.

### CLI on the user's computer

Install the runtime with the host-specific path above, on the computer that holds the library. When Grok Bot runs commands on the user's Mac, that computer is the Mac. Pass `--agent-name grok-bot` on each command, or export `WENLAN_AGENT_NAME=grok-bot` for the shell. Without a name, the daemon records the caller as `unknown`.

```bash
wenlan --agent-name grok-bot recall "what we decided"
wenlan --agent-name grok-bot memories
wenlan --agent-name grok-bot spaces list
wenlan --agent-name grok-bot capture "the decision, in one sentence"
```

`recall` searches memories and the pages that match. `memories` lists recent memories. `spaces list` lists Spaces. `capture` stores one memory and prints its id (`Stored memory <id>`).

A command run on Grok Bot's own Linux computer talks to a daemon on that computer. It does not see the library on the user's Mac.

### MCP on the same computer

Use this when the client starts `wenlan-mcp` on the same computer as the daemon. The connector calls `http://127.0.0.1:7878` on the machine where its process runs.

```json
{
  "mcpServers": {
    "wenlan": {
      "command": "wenlan-mcp",
      "args": ["--agent-name", "grok-bot"]
    }
  }
}
```

When `wenlan-mcp` is not on `PATH`, set `command` to `~/.wenlan/bin/wenlan-mcp` and keep the same `args`. Or set `command` to `npx` and `args` to `["-y", "wenlan-mcp", "--agent-name", "grok-bot"]`.

A stdio server that starts on Grok Bot's Linux computer cannot open the daemon on the user's Mac. For that library, use the CLI on the Mac.

### Verify

On the computer where the daemon runs:

```bash
wenlan status
wenlan --agent-name grok-bot capture "Wenlan Grok Bot setup check. Disposable."
wenlan --agent-name grok-bot recall "Wenlan Grok Bot setup check"
```

The round trip is proven when `wenlan status` reports a healthy runtime and `recall` returns the captured sentence. `capture` prints `Stored memory <id>`. Delete that memory with the MCP `forget` tool, with `memory_id` set to `<id>`. The `wenlan` CLI has no forget command.

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

Then use the current client's Wenlan tools to capture a disposable sentence and recall it. Delete the test memory afterward if the client exposes `forget`. If the tools are not visible, start a new client session and verify again before declaring setup complete. For Grok Bot, run the CLI round trip in [Grok Bot](#grok-bot).
