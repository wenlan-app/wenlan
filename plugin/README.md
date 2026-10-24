# Wenlan

A living wiki your AI tools write, keep current, and cite. Pages link to their sources and update when those change.

Wenlan is local-first. Memories, pages, and session notes live on your machine under `~/.wenlan`, served by a small local daemon. Nothing in this plugin is stored by Anthropic or by the plugin itself.

## What the plugin contains

- **Skills**: one front door, `/wenlan`, plus `/brief`, `/capture`, `/recall`, and `/handoff`. Setup, notes, distill, wiki, lint, curate, and forget are reached by saying them after `/wenlan` and stay out of the slash menu. Each skill talks to the local daemon over HTTP on `127.0.0.1:7878`.
- **One local MCP server** (`wenlan`), started by `plugin/scripts/wenlan-mcp-runner.sh`. It exposes memory tools (capture, recall, brief, distill, and friends) to Claude Code and Cowork. Claude chat on the web ignores local MCP servers.
- **Two `SessionStart` hooks.** `plugin/hooks/session-context.sh` reads the session id Claude Code passes it, exports `WENLAN_SESSION=claude-code:<id>` for the commands Claude runs, and prints one line so Claude can pass that session when it saves a memory, note, or handoff; Wenlan then shows which conversation a memory or note came from. It makes no network call. `plugin/hooks/check-daemon.sh` probes the local daemon, tells a running daemon that Claude Code is connected (the same `POST /api/agents` the app sends when you connect a tool there, so Claude's first recall already sees your pages; a tool you changed or turned off in Connections stays as it is), and prints at most a few lines: a reminder if the daemon is not running, a count of handoff notes waiting to sync (from the local `wenlan outbox status` command, parsed with `python3`), and a notice if the daemon and plugin versions have drifted apart. It never blocks a session and never installs anything.

## Setup

Install from the Claude directory, or from the Claude Code marketplace:

```text
/plugin marketplace add wenlan-app/wenlan
/plugin install wenlan@7xuanlu-wenlan
```

Then restart Claude Code and run `/wenlan`. When Wenlan is not ready yet it starts setup, which installs the local runtime if it is missing (macOS Apple Silicon; elsewhere it links the install guide), configures local memory, verifies the MCP round trip, and prints "Wenlan ready". After that, `/capture <something to remember>` or `/brief` is the daily loop.

The first run downloads an embedding model of roughly 210 MB, so allow a few minutes.

## What it runs and where data goes

Everything the plugin does is listed here so you can decide whether to install it.

| Component | What it runs | Network destination |
|---|---|---|
| Skills and MCP server | Read and write memories, pages, and session notes | `127.0.0.1:7878` (the local Wenlan daemon on your machine) |
| `/wenlan` setup | When the runtime is missing, runs `npx -y wenlan@<pinned version> setup` (macOS Apple Silicon). On other platforms it stops and points you to the install guide | `registry.npmjs.org` (npm package `wenlan`), which downloads release binaries from `github.com/wenlan-app/wenlan/releases` (GitHub serves the files from `objects.githubusercontent.com`) |
| MCP runner fallback | If no local `wenlan-mcp` binary exists, runs `npx -y wenlan-mcp@<pinned version>` | `registry.npmjs.org` (npm package `wenlan-mcp`) |
| `SessionStart` hooks | One health probe of the local daemon, plus the local outbox and version checks above | `127.0.0.1:7878` only (the session hook makes no network call) |
| Local daemon, first start | Downloads the search embedding model (about 210 MB, `Qdrant/bge-base-en-v1.5-onnx-Q`) once | `huggingface.co`. No memory content is sent. If a Hugging Face token is already saved on the machine, the download library attaches it |

No memory content, prompts, or file contents leave your machine through this plugin. The daemon's full privacy notes are in [docs/PRIVACY.md](../docs/PRIVACY.md). Steep, the model-backed background enrichment (classification, entity extraction, page synthesis, reranking), is opt-in and configured in the daemon with `wenlan steep configure`, not in the plugin.

The MCP runner picks a binary in this order: the `WENLAN_MCP_DEV_BIN` environment variable, a local override file next to the script, the installed `~/.wenlan/bin/wenlan-mcp`, then the pinned npm package. The runner reads `$HOME` to find the installed binary, which is why a reviewer may hold the local MCP command for a manual look.

## Where your data lives

```text
~/.wenlan/pages/               wiki pages distilled from memories (Markdown)
~/.wenlan/sessions/            session logs by date (Markdown)
~/.wenlan/sessions/_status/    current per-project goals and last-handoff timestamp
~/.wenlan/bin/                 installed binaries
```

Memories and the graph live in a local database; location in [docs/setup-and-data.md](../docs/setup-and-data.md).

Browse with any editor, or symlink `~/.wenlan/pages/` into an Obsidian vault for the graph view. No desktop app is required.

## Local memory and agent-side model phases

By default setup configures local memory: no model download beyond the embedder, no API key, no prompts. The daemon stores, embeds, dedupes, and serves hybrid search. Where the daemon would normally call a model, the skill asks Claude itself to do the equivalent step and posts the result back over HTTP:

| Phase | Model-equipped daemon | Local memory plus skill |
|---|---|---|
| Pick `memory_type` | daemon classifier | `/capture` picks one of 6 types from content |
| Extract entities and relations | daemon `extract.rs` | `/capture` posts to `/api/memory/entities` and `/api/memory/relations` |
| Synthesize a page | daemon distill cycles | `/wenlan distill` reads the cluster, writes the page, posts to `/api/pages` |
| Steep out of date pages | automatic steep | `/wenlan steep` claims each stale page, rewrites it from its sources, and commits through `/api/steep/commit` |
| Expand a query or rerank hits | daemon expansion and rerank | `/recall` rewrites the query before search and reorders hits after |

The daemon stays the single writer and storage owner. Claude does the thinking.

## Daily commands

<!-- verbs:start -->
<!-- generated from plugin/skills/verbs.json by scripts/gen-plugin-verbs.mjs; do not edit -->

- `/wenlan [what you want]` - the front door: say what you want, or type it bare for status and setup
- `/brief [topic]` - load the Brief for the current Space (optional; not a boot step)
- `/capture <x>` - save one durable memory in flow, or this conversation as one note on request
- `/recall <q>` - search local memory
- `/handoff` - end-of-session debrief: captures, session log, Brief update
- `/wenlan setup` - set up or repair Wenlan (installs the local runtime)
- `/wenlan lint [deep|repair] [scope]` - diagnose, or resolve all findings safely
- `/wenlan distill [t]` - advanced: synthesize or refresh source-backed pages for the active Space
- `/wenlan steep [space:<name>]` - write pages from new material and bring out of date ones up to date with this AI tool (steep on demand)
- `/wenlan wiki [space:<name>]` - read this repository and write a wiki about it, sized to the repository, each page tied to the code lines it relies on
- `/wenlan pages [q]` - browse and open distilled pages
- `/wenlan curate <surface>` - deep audit of captures, revisions or refinements
- `/wenlan forget <id>` - delete one memory by ID, after confirmation
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
