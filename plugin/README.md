# Wenlan

Docs, skills and rules your AI tools keep current and work from. Each doc links to its sources. Open source, and works with your existing Obsidian vault.

Wenlan is local-first. Memories, pages, and session notes live on your machine under `~/.wenlan`, served by a small local daemon. Nothing in this plugin is stored by Anthropic or by the plugin itself.

## What the plugin contains

- **Skills**: one front door, `/wenlan`, plus `/brief`, `/capture`, `/recall`, and `/handoff`. Setup, page lookup, repository docs, lint, curate, and forget are reached by saying them after `/wenlan` and stay out of the slash menu. Knowledge organization and maintenance share the Steep (沉澱) workflow. Each skill talks to the local daemon over HTTP on `127.0.0.1:7878`.
- **One local MCP server** (`wenlan`), started by `plugin/scripts/wenlan-mcp-runner.sh`. It exposes memory tools, including the internal `distill` operation, to Claude Code and Cowork. User-directed knowledge organization and maintenance use the Steep Skill as their guided workflow. Claude chat on the web ignores local MCP servers.
- **Three `SessionStart` hooks.** `session-context.sh` carries Claude Code's session id into Wenlan commands, `check-daemon.sh` reports local runtime or version problems without blocking startup, and `brief-context.sh` quietly brings in each current Brief item's id and one line (about 2,000 tokens at most). The Brief hook prints nothing when Wenlan is unavailable.
- **A `UserPromptSubmit` hook and two `Stop` hooks.** `prompt-context.sh` and `stop-check.sh` provide the optional in-chat check against earlier decisions. `handoff-reminder.sh` reads the latest assistant usage from Claude Code's local transcript and, once per session after 80%, asks Claude to run the Wenlan handoff before finishing the turn. The context window is the first of `WENLAN_CONTEXT_WINDOW` or `CLAUDE_CODE_AUTO_COMPACT_WINDOW` that is set, else 1M for a `[1m]` model or usage above 200k, and 200k otherwise. These variables apply to Claude Code only: in Codex the reminder reads `model_context_window` from the Codex session file. All stay local and fail quietly when their inputs or Wenlan are unavailable.

## Setup

Install from the Claude directory, or from the Claude Code marketplace:

```text
/plugin marketplace add wenlan-app/wenlan
/plugin install wenlan@7xuanlu-wenlan
```

Then restart Claude Code and run `/wenlan`. When Wenlan is not ready yet it starts setup, which installs the local runtime if it is missing (macOS Apple Silicon, Linux x64 or ARM64, Windows x64; elsewhere it links the install guide), configures local memory, verifies the MCP round trip, and prints "Wenlan ready". After that, `/capture <something to remember>` or `/brief` is the daily loop.

The first run downloads an embedding model of roughly 210 MB, so allow a few minutes.

## What it runs and where data goes

Everything the plugin does is listed here so you can decide whether to install it.

| Component | What it runs | Network destination |
|---|---|---|
| Skills and MCP server | Read and write memories, pages, and session notes | `127.0.0.1:7878` (the local Wenlan daemon on your machine) |
| `/wenlan` setup | When the runtime is missing, runs `npx -y wenlan@<pinned version> setup` (macOS Apple Silicon, Linux x64 or ARM64, Windows x64). On other platforms it stops and points you to the install guide | `registry.npmjs.org` (npm package `wenlan`), which downloads release binaries from `github.com/wenlan-app/wenlan/releases` (GitHub serves the files from `objects.githubusercontent.com`) |
| MCP runner fallback | If no local `wenlan-mcp` binary exists, runs `npx -y wenlan-mcp@<pinned version>` | `registry.npmjs.org` (npm package `wenlan-mcp`) |
| `SessionStart` hooks | Health and version checks, session identity, and the current Brief's one-line items | `127.0.0.1:7878` only (the session-id hook makes no network call) |
| `UserPromptSubmit` and `Stop` hooks | Optional decision checks, plus one local 80% handoff reminder per session | `127.0.0.1:7878` only; reminder state stays under the Wenlan data directory or temporary directory |
| Local daemon, first start | Downloads the search embedding model (about 210 MB, `Qdrant/bge-base-en-v1.5-onnx-Q`) once | `huggingface.co`. No memory content is sent. If a Hugging Face token is already saved on the machine, the download library attaches it |

No memory content, prompts, or file contents leave your machine through this plugin. The daemon's full privacy notes are in [docs/PRIVACY.md](../docs/PRIVACY.md). Automatic Steep’s model-backed background work is opt-in and configured in the daemon with `wenlan steep configure`. On-demand Steep uses the connected AI tool; user-directed reorganization runs only when requested.

The MCP runner picks a binary in this order: the `WENLAN_MCP_DEV_BIN` environment variable, a local override file next to the script, the installed `~/.wenlan/bin/wenlan-mcp`, then the pinned npm package. The runner reads `$HOME` to find the installed binary, which is why a reviewer may hold the local MCP command for a manual look.

## Where your data lives

```text
~/.wenlan/pages/               wiki pages distilled from memories (Markdown)
~/.wenlan/sessions/            older session logs by date (Markdown; no longer written)
~/.wenlan/sessions/_status/    current per-project goals and last-handoff timestamp
~/.wenlan/bin/                 installed binaries
```

Memories and the graph live in a local database; location in [docs/setup-and-data.md](../docs/setup-and-data.md).

Browse with any editor, or symlink `~/.wenlan/pages/` into an Obsidian vault for the graph view. No desktop app is required.

## Local memory and agent-side model phases

By default setup configures local memory: no model download beyond the embedder, no API key, no prompts. The daemon stores, embeds, dedupes, and serves hybrid search. When you request on-demand work, the connected AI reads the relevant Skill procedure and uses MCP tools; it may use a small AI helper for a bounded enrichment or judgment step. The daemon owns queues, claim tokens, guarded writes, and the single-writer path. Automatic work instead uses the models configured for daemon background jobs; it is not dispatched to arbitrary connected AI tools:

| Phase | Model-equipped daemon | Local memory plus skill |
|---|---|---|
| Pick `memory_type` | daemon classifier | `/capture` picks one of 6 types from content |
| Extract entities and relations | daemon `extract.rs` | `/capture` posts to `/api/memory/entities` and `/api/memory/relations` |
| Synthesize or refresh a page | daemon distill cycles | Steep handles the requested page work and posts through the page API; distillation is an internal operation, not a separate workflow |
| Maintain knowledge around a goal | Requires an explicit user request | `/wenlan steep` follows the request and updates the relevant memories, pages, folders, topics, or relationships |
| Expand a query or rerank hits | daemon expansion and rerank | `/recall` rewrites the query before search and reorders hits after |

The daemon stays the single writer and storage owner. The connected AI handles on-demand reasoning. In automatic mode, the daemon processes eligible background work with its configured models; it does not invent a goal-driven reorganization request.

## Daily commands

<!-- verbs:start -->
<!-- generated from plugin/skills/verbs.json by scripts/gen-plugin-verbs.mjs; do not edit -->

- `/wenlan [what you want]` - the front door: say what you want, or type it bare for status and setup
- `/brief [topic]` - load the Brief for the current Space (optional; not a boot step)
- `/capture <x>` - save one durable memory in flow, or this conversation as one note on request
- `/recall <q>` - search local memory
- `/handoff` - end of session: update the Brief and the pages this session used
- `/how-i-work` - read this Space's approved rules before choosing an approach
- `/wenlan setup` - set up or repair Wenlan (installs the local runtime)
- `/wenlan lint [deep|repair] [scope]` - diagnose, or resolve all findings safely
- `/wenlan steep [request] [space:<name>]` - prepare new material, update pages, or arrange existing knowledge around your goal
- `/wenlan docs [space:<name>]` - read this repository and write docs about it, sized to the repository, each page tied to the code lines it relies on (earlier `/wenlan wiki`)
- `/wenlan pages [q]` - browse and open distilled pages
- `/wenlan curate <surface>` - deep audit of captures, revisions or refinements
- `/wenlan forget <id>` - delete one memory by ID, after confirmation
- `/wenlan theme [idea]` - design a custom look for the app: colors, fonts, corners, images
<!-- verbs:end -->

Skill instructions live in [`skills/`](skills/). Plugin metadata is in [`.claude-plugin/plugin.json`](.claude-plugin/plugin.json), the MCP declaration in [`.mcp.json`](.mcp.json), and the marketplace entry in [`../.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json) at the repository root.

## Developing the plugin

To point the MCP server at a locally built binary, create the gitignored override file:

```text
cargo build -p wenlan-mcp --release
ln -s $(pwd)/target/release/wenlan-mcp plugin/scripts/wenlan-mcp.local
```

Reload the plugin (`/reload-plugins`) and the runner uses the local binary on the next MCP spawn. `../scripts/dev-sync.sh` does the same for a debug build. The npm pin in the runner is rewritten by `../scripts/bump-version.sh` on every release and checked by `../scripts/validate-versions.sh`.

## Links

- [wenlan.app](https://wenlan.app): project home
- [wenlan.app/learn/claude-code-memory](https://wenlan.app/learn/claude-code-memory): Claude Code memory concept article
- [wenlan.app/docs/daily-workflow](https://wenlan.app/docs/daily-workflow): the brief, capture, recall, handoff loop
- [wenlan.app/docs/get-started](https://wenlan.app/docs/get-started): install and verify
- [Privacy](https://github.com/wenlan-app/wenlan/blob/main/docs/PRIVACY.md): privacy policy, covering what Wenlan stores locally and what can leave your machine

## License

Apache-2.0.
