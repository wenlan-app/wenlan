# wenlan-cli

Wenlan's command line. Wenlan is a personal wiki your AI tools keep up to date; the CLI is how you capture, recall, hand off, and keep the pages healthy from a terminal, plus set up and check the local runtime.

License: Apache-2.0.

## Install

Recommended user setup on macOS Apple Silicon:

```bash
npx -y wenlan setup
```

The setup package downloads `wenlan`, `wenlan-server`, and `wenlan-mcp` into `~/.wenlan/bin/`, configures local memory, starts the background runtime, and verifies status. It currently supports macOS Apple Silicon.

Linux x64/ARM64 has an automated shell path:

```bash
curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash
export PATH="$HOME/.wenlan/bin:$PATH"
wenlan setup --basic
wenlan background on
wenlan status
```

Windows x64 users can unpack `wenlan-windows-x64.zip` from [Releases](https://github.com/wenlan-app/wenlan/releases/latest) into a directory on `PATH`. Keep the bundled `onnxruntime.dll` beside the three `.exe` files, then run the same three `wenlan` commands.

### macOS Intel

There is currently no supported complete-runtime install for macOS Intel. The pinned ONNX Runtime dependency does not publish an x86_64 macOS binary, so a custom build requires compiling ONNX Runtime separately and providing it through `ORT_LIB_LOCATION`. A plain `cargo build` is not a working installation path.

For CLI-only local development:

```bash
cargo install --path crates/wenlan-cli
```

Or build only the CLI from the workspace:

```bash
cargo build -p wenlan --release
./target/release/wenlan --help
```

## Configuration

Set `WENLAN_HOST` to point at a remote daemon:

```bash
export WENLAN_HOST=http://127.0.0.1:7878  # default
```

A bare port (`17917`) or `host:port` is completed to `http://127.0.0.1:17917` /
`http://host:port`; a loopback shorthand takes part in autostart like the full URL.

When a request to a loopback daemon cannot connect, the CLI automatically starts
the registered background service (`com.wenlan.server`) and waits for its health
endpoint. Remote or invalid `WENLAN_HOST` values keep the original connection
error. `status`, `background`, and `restart` skip autostart; set
`WENLAN_NO_AUTOSTART=1` to opt out for other commands.

## Common tasks

Hand-written notes and examples for the commands most people reach for. The complete, generated reference for every command and flag follows in [Subcommands](#subcommands).

### Daily

**`wenlan recall <query>`**

Search memories (vector + FTS hybrid) and return a ranked list plus the compiled pages that match (`POST /api/memory/search`). `--limit` caps the memory results; matching pages are listed separately and do not count against it.

```bash
wenlan recall "what we agreed on for the API"
wenlan recall "embedding model" --limit 5
wenlan recall "memory layer" --format json | jq '.results[].score'
```

**`wenlan brief`**

Read the current Space Brief, optionally with related context.

```bash
wenlan brief
```

**`wenlan capture`**

Store a memory. Provide content positionally, via `--file`, or pipe via stdin.

```bash
wenlan capture "remember this insight" --type fact
wenlan capture --file notes.md --type decision
echo "stdin pipe content" | wenlan capture --type lesson
```

**`wenlan memories`**

List recent memories.

```bash
wenlan memories
wenlan memories --limit 5
wenlan memories --type fact --format json
wenlan memories --pending
```

**`wenlan pages [query]`**

Browse distilled pages, or open one in your editor by title query.

```bash
wenlan pages
wenlan pages "project roadmap"
wenlan pages "project roadmap" --print
```

`--print` writes the page text and the memories it was written from to the terminal instead of opening it. If the query matches more than one page, it lists them and asks you to narrow the query. With `--format json` it prints one object with `page` and `sources`.

**`wenlan curate`**

Walk pending revisions (conflicts / merges) awaiting your accept or dismiss.

```bash
wenlan curate
```

**`wenlan lint`**

Check the health of memories, pages, and the running Wenlan; report, then resolve findings safely.

```bash
wenlan lint
```

**`wenlan spaces`**

Manage memory spaces (buckets).

```bash
wenlan spaces list
wenlan spaces add ideas --default
wenlan spaces show career
wenlan spaces default work
wenlan spaces move scratch career
```

### Setup

**`wenlan status`**

Answers whether Wenlan is running: the background service, its health, and the Steep state. `wenlan doctor` has the full diagnosis (models, API keys, providers).

```bash
wenlan status
wenlan status --format json
```

**`wenlan setup`**

Configure Wenlan's runtime mode.

```bash
wenlan setup                  # interactive
wenlan setup --basic          # local memory, no local model or API key
wenlan setup --model qwen3-4b # download/select a local model
wenlan setup --anthropic-api-key-env ANTHROPIC_API_KEY
```

**`wenlan background <on|off>` and `wenlan restart`**

Start or stop the per-user background runtime. `wenlan background off` stops the current daemon while preserving its login registration and writes `<data root>/autostart.off`, so recovery will not start it again. Run `wenlan background on` to remove the marker and start it again; it waits up to 10 s for the daemon to answer before saying "started" and, on macOS, reports an error the daemon logged during this start (never one left over from an earlier run). Most users run `wenlan background on` once after setup and `wenlan restart` after upgrades installed outside `wenlan setup`.

```bash
wenlan background on
wenlan background off
wenlan restart
```

**`wenlan steep <status|configure|disable>`**

Models and provider keys are capability only; they do not authorize automatic
model calls. Inspect the two job groups, confirm exact hard-pinned sources, or
turn model-backed background work off while keeping providers installed:

```bash
wenlan steep status
wenlan steep configure --everyday on-device --synthesis on-device
wenlan steep disable
```

`configure` shows what each job does and discloses cloud cost/data transfer or
on-device CPU/GPU/RAM use before confirmation. An unavailable hard pin pauses;
Wenlan never substitutes another provider. Deterministic storage, indexing, and
sync continue when Steep is off. `wenlan enrichment` still works as a hidden alias; its removal, together with the hidden `search` subcommand, is tracked in [wenlan-app/wenlan#859](https://github.com/wenlan-app/wenlan/issues/859).

**`wenlan doctor`**

Diagnose runtime reachability, background mode, model setup, and API key setup.

```bash
wenlan doctor
```

**`wenlan models`**

Manage opt-in local models.

```bash
wenlan models list
wenlan models status
wenlan models install qwen3-4b
wenlan models reranker lite
```

**`wenlan keys`**

Manage provider API keys.

```bash
wenlan keys status
wenlan keys set anthropic --env ANTHROPIC_API_KEY
wenlan keys clear anthropic
```

**`wenlan connect <client>`**

Configure Wenlan MCP for a supported client. This is the MCP-only path for Claude Code users who do not want the plugin, and for Codex, Cursor, Claude Desktop, VS Code, and Gemini CLI.

```bash
wenlan connect claude-code
wenlan connect codex
wenlan connect cursor --dry-run
```

Supported clients: `claude-code`, `codex`, `gemini`, `cursor`, `claude-desktop`, `vscode`.

When `wenlan-mcp` is installed next to the `wenlan` CLI, the generated config points at that binary. Otherwise it falls back to `npx -y wenlan-mcp`. Use `--dry-run` to preview JSON config edits before writing them.

### Maintenance

**`wenlan sources add <path>`**

Register or resync a file or folder source.

```bash
wenlan sources add ~/Notes
wenlan sources add ~/Notes/project.md
```

**`wenlan outbox <status|drain>`**

Inspect or sync notes saved locally while Wenlan was not running.

```bash
wenlan outbox status
wenlan outbox drain
```

**`wenlan entities <list|archive|restore>`**

See the people, projects, and topics Wenlan has found, and archive or restore them by id.

```bash
wenlan entities list --status detected --limit 20
wenlan entities archive ent_123 ent_456 --dry-run
wenlan entities archive ent_123 ent_456
wenlan entities restore ent_123
```

<!-- cli-reference:start -->
<!-- generated from the clap definitions in crates/wenlan-cli/src/main.rs by `WENLAN_UPDATE_CLI_REFERENCE=1 cargo test -p wenlan --bin wenlan cli_reference`; do not edit -->
## Subcommands

This section is generated from the same definitions that produce `wenlan --help` and `wenlan <command> --help`. Hidden aliases are not listed. These global options are parsed for every subcommand; commands that do not work on a Space reject `--space` and `--all-spaces` at runtime:

- `--format <FORMAT>`: Output format. Auto-detects JSON when piped, table on TTY. Possible values: `auto` (Pick automatically based on stdout TTY: terminal → Table, pipe → Json), `json`, `table`. Default: `auto`.
- `-q, --quiet`: Suppress all non-error output. Useful for scripts.
- `--agent-name <AGENT_NAME>`: Identify the caller in X-Agent-Name audit metadata.
- `--space <SPACE>`: Use one registered Space for scope-aware reads and writes.
- `--all-spaces`: Read across every Space. Invalid for writes and strict WENLAN_SPACE pins.

**Daily**

### `wenlan brief`

Read the current Space Brief, optionally with related context.

```text
wenlan brief [OPTIONS] [TOPIC] [COMMAND]
```

Arguments:

- `[TOPIC]`: Optional topic for a separately-labelled same-Space recall.

#### `wenlan brief update`

Apply a typed item-level handoff update from JSON.

```text
wenlan brief update [OPTIONS] --file <JSON_FILE>
```

Options:

- `--file <JSON_FILE>`: JSON file containing a BriefUpdateRequest.

#### `wenlan brief history`

List past Brief updates, newest first.

```text
wenlan brief history [OPTIONS]
```

Options:

- `-l, --limit <LIMIT>`: Maximum number of updates to list. Default: `20`.

#### `wenlan brief show`

Show the Brief as it stood after one past update.

```text
wenlan brief show [OPTIONS] --version <VERSION>
```

### `wenlan capture`

Capture a memory. Provide text positionally, or use --file, or pipe via stdin.

```text
wenlan capture [OPTIONS] [TEXT]
```

Arguments:

- `[TEXT]`: Content text. If omitted and --file unset, read from stdin.

Options:

- `-f, --file <FILE>`: Read content from a file.
- `-t, --type <MEMORY_TYPE>`: Memory type (e.g. fact, decision).

### `wenlan recall`

Recall scored memories for a query, plus the compiled pages that match it.

```text
wenlan recall [OPTIONS] <QUERY>
```

Arguments:

- `<QUERY>`: Query to recall memories for.

Options:

- `-l, --limit <LIMIT>`: Max memories to return. Default: `10`.

### `wenlan pages`

Browse distilled pages, open one in your editor by title query, or lock a page so machine updates wait for your review.

```text
wenlan pages [OPTIONS] [QUERY]
       wenlan pages <COMMAND>
```

Arguments:

- `[QUERY]`: Title/filename substring. Omit to list pages newest-first.

Options:

- `-l, --limit <LIMIT>`: Max pages to list (newest-first). 0 = all. Ignored when a query opens a page. Default: `20`.
- `--resolve-id`: Print the matched page's stable internal id instead of opening it.
- `--print`: Print the matched page's text and its sources instead of opening it.

#### `wenlan pages lock`

Require review for every machine write to a page: agent and pipeline updates are staged as revision cards. Your own edits still apply.

```text
wenlan pages lock [OPTIONS] <PAGE_ID>
```

Arguments:

- `<PAGE_ID>`: The page's id (see `wenlan pages <query> --resolve-id`).

#### `wenlan pages unlock`

Clear a page's review lock.

```text
wenlan pages unlock [OPTIONS] <PAGE_ID>
```

Arguments:

- `<PAGE_ID>`: The page's id (see `wenlan pages <query> --resolve-id`).

### `wenlan memories`

List recent memories.

```text
wenlan memories [OPTIONS]
```

Options:

- `-l, --limit <LIMIT>`: Max results. Default: `20`.
- `-t, --type <MEMORY_TYPE>`: Filter by memory type.
- `--pending`: Only pending captures (not yet confirmed; what the MCP list_pending tool returns).

### `wenlan curate`

Walk pending revisions (conflicts / merges) awaiting your accept or dismiss.

```text
wenlan curate [OPTIONS] [COMMAND]
```

#### `wenlan curate revisions`

List pending revisions (the default when no action is given).

```text
wenlan curate revisions [OPTIONS]
```

Options:

- `-l, --limit <LIMIT>`: Max revisions to fetch. Default: `20`.

#### `wenlan curate accept`

Accept a revision: replace the original memory with the revised text.

```text
wenlan curate accept [OPTIONS] <REVISION_SOURCE_ID>
```

Arguments:

- `<REVISION_SOURCE_ID>`: `revision_source_id` of the revision (from the list output).

#### `wenlan curate dismiss`

Dismiss a revision: drop it, keep the original memory.

```text
wenlan curate dismiss [OPTIONS] <REVISION_SOURCE_ID>
```

Arguments:

- `<REVISION_SOURCE_ID>`: `revision_source_id` of the revision (from the list output).

### `wenlan lint`

Check the health of memories, pages, and the running Wenlan; report, then resolve findings safely.

```text
wenlan lint [OPTIONS]
```

Options:

- `--profile <PROFILE>`
- `--allow-external`: Permit a deep semantic pass to use an already configured external provider.
- `--agent-assist`: Return bounded high-recall semantic candidates for the calling agent.
- `--agent-submission <JSON_FILE>`: Submit typed agent verdicts produced from a prior prepare report.

### `wenlan spaces`

Manage Spaces, the separate wikis Wenlan keeps (list, add, default, move, show).

```text
wenlan spaces [OPTIONS] <COMMAND>
```

#### `wenlan spaces list`

List all registered spaces.

```text
wenlan spaces list [OPTIONS]
```

#### `wenlan spaces add`

Register a new space.

```text
wenlan spaces add [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: Space name (e.g. "career", "health", "ideas").

Options:

- `--default`: Also set this space as the default.

#### `wenlan spaces default`

Get or set the default space.

```text
wenlan spaces default [OPTIONS] [NAME]
```

Arguments:

- `[NAME]`: Space name to set as default. Omit to print the current default.

Options:

- `--clear`: Clear the daemon-owned default save space.

#### `wenlan spaces move`

Bulk-reassign all memories from one space to another.

```text
wenlan spaces move [OPTIONS] <FROM> <TO>
```

Arguments:

- `<FROM>`: Source space.
- `<TO>`: Destination space.

#### `wenlan spaces show`

Show detail for a space — name, description, memory count, entity count, default + starred flags.

```text
wenlan spaces show [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: Space name.

**Setup**

### `wenlan setup`

Guided setup: get Wenlan running, optionally with a local model or an Anthropic key.

```text
wenlan setup [OPTIONS]
```

Options:

- `--basic`: Set up without a model or API key.
- `--model <MODEL_ID>`: Download and select a local model, for example qwen3-4b.
- `--anthropic-api-key-env <ENV_VAR>`: Read an Anthropic key from this environment variable.
- `-y, --yes`: Skip confirmation prompts where possible.

### `wenlan status`

Check whether Wenlan is running.

```text
wenlan status [OPTIONS]
```

### `wenlan doctor`

Diagnose runtime, model, and API key setup.

```text
wenlan doctor [OPTIONS]
```

### `wenlan background`

Control whether Wenlan keeps running in the background.

```text
wenlan background [OPTIONS] <COMMAND>
```

#### `wenlan background on`

Start Wenlan now and keep it running in the background after login.

```text
wenlan background on [OPTIONS]
```

#### `wenlan background off`

Stop Wenlan now while keeping its background registration.

```text
wenlan background off [OPTIONS]
```

### `wenlan restart`

Restart the Wenlan background process. Required after an update.

```text
wenlan restart [OPTIONS]
```

### `wenlan connect`

Connect Wenlan to a supported agent or editor.

```text
wenlan connect [OPTIONS] <CLIENT>
```

Arguments:

- `<CLIENT>`: Client to configure. Possible values: `claude-code` (Claude Code without the Wenlan plugin), `codex` (OpenAI Codex CLI), `gemini` (Gemini CLI), `cursor` (Cursor editor), `claude-desktop` (Claude Desktop), `vscode` (VS Code workspace MCP config).

Options:

- `--dry-run`: Print the command or file edit without changing anything.

### `wenlan models`

Manage local models.

```text
wenlan models [OPTIONS] <COMMAND>
```

#### `wenlan models list`

List local models Wenlan can download and run.

```text
wenlan models list [OPTIONS]
```

#### `wenlan models status`

Show selected/downloaded local model state.

```text
wenlan models status [OPTIONS]
```

#### `wenlan models install`

Download and select a local model.

```text
wenlan models install [OPTIONS] [MODEL_ID]
```

Arguments:

- `[MODEL_ID]`: Model id, for example qwen3-4b.

Options:

- `-y, --yes`: Skip confirmation before downloading.

#### `wenlan models reranker`

Set the cross-encoder reranker mode (off/lite/full).

```text
wenlan models reranker [OPTIONS] <MODE>
```

Arguments:

- `<MODE>`: Mode to persist; the daemon reads it at startup. Possible values: `off` (No cross-encoder on any path (default)), `lite` (Turbo CE (jina-turbo, ~146MB) on quick + context + explicit deep rerank), `full` (Turbo on the light paths; heavy bge-base (~1.1GB) on the deep rerank).

### `wenlan keys`

Manage provider API keys.

```text
wenlan keys [OPTIONS] <COMMAND>
```

#### `wenlan keys status`

Show API key status.

```text
wenlan keys status [OPTIONS]
```

#### `wenlan keys set`

Store an API key.

```text
wenlan keys set [OPTIONS] <PROVIDER>
```

Arguments:

- `<PROVIDER>`: Provider to configure. Possible values: `anthropic`.

Options:

- `--env <ENV_VAR>`: Read the key from this environment variable instead of prompting.

#### `wenlan keys clear`

Clear a stored API key.

```text
wenlan keys clear [OPTIONS] <PROVIDER>
```

Arguments:

- `<PROVIDER>`: Provider to clear. Possible values: `anthropic`.

### `wenlan steep`

Configure, inspect, or disable Steep, the optional background upkeep (model-backed).

```text
wenlan steep [OPTIONS] <COMMAND>
```

#### `wenlan steep status`

Show the effective source and ready/paused/off state for each job group.

```text
wenlan steep status [OPTIONS]
```

#### `wenlan steep configure`

Confirm exact sources and enable model-backed background work.

```text
wenlan steep configure [OPTIONS] --everyday <EVERYDAY> --synthesis <SYNTHESIS>
```

Options:

- `--everyday <EVERYDAY>`: Source for background organization. Possible values: `anthropic`, `external`, `on-device`.
- `--synthesis <SYNTHESIS>`: Source for page synthesis. Possible values: `anthropic`, `external`, `on-device`.
- `-y, --yes`: Confirm the mapping supplied on this command line.

#### `wenlan steep disable`

Turn off all model-backed background work without removing providers.

```text
wenlan steep disable [OPTIONS]
```

**Maintenance**

### `wenlan sources`

Manage folders and files Wenlan should learn from.

```text
wenlan sources [OPTIONS] <COMMAND>
```

#### `wenlan sources add`

Add a folder or file source and sync it now.

```text
wenlan sources add [OPTIONS] <PATH>
```

Arguments:

- `<PATH>`: Path to a directory or file to add as a source.

Options:

- `--type <SOURCE_TYPE>`: Kind of source. `okf` imports an OKF bundle (for example an OpenWiki folder) into one Space; pass --space to choose it. Possible values: `directory` (A folder or single file of documents), `okf` (An OKF knowledge bundle). Default: `directory`.

#### `wenlan sources save`

Save one document read elsewhere (an email, a Notion or Drive page, a Slack thread, an article) with its link, so notes cite the original. Text comes from --file or stdin. Saving the same link again replaces the saved copy; Wenlan does not re-fetch the original.

```text
wenlan sources save [OPTIONS] --url <URL> --title <TITLE>
```

Options:

- `--url <URL>`: Permanent http(s) link to the original.
- `--title <TITLE>`: Title of the original.
- `--label <LABEL>`: Where it came from, e.g. gmail, notion, drive, slack, github, web.
- `--file <FILE>`: Read the text from this file instead of stdin.
- `--create-only`: Refuse if this link is already saved instead of replacing it.

### `wenlan outbox`

Inspect or sync notes saved locally while Wenlan was not running.

```text
wenlan outbox [OPTIONS] <COMMAND>
```

#### `wenlan outbox status`

List queued and failed envelope files without contacting the daemon.

```text
wenlan outbox status [OPTIONS]
```

#### `wenlan outbox drain`

Ask the daemon to replay queued envelopes.

```text
wenlan outbox drain [OPTIONS]
```

### `wenlan export`

Export pages to an external bundle format.

```text
wenlan export [OPTIONS] <COMMAND>
```

#### `wenlan export okf`

Export pages as a pure OKF v0.2 bundle (index.md, pages/, sources/). Pass --space to limit the bundle to one Space.

```text
wenlan export okf [OPTIONS] <DIR>
```

Arguments:

- `<DIR>`: Target directory. Created when missing; otherwise it must be empty or hold a previous Wenlan OKF export.

### `wenlan entities`

List, archive, restore, merge, or alias knowledge-graph entities.

```text
wenlan entities [OPTIONS] <COMMAND>
```

#### `wenlan entities list`

List entities in every state, with their type, status, and memory count.

```text
wenlan entities list [OPTIONS]
```

Options:

- `--type <ENTITY_TYPE>`: Only this entity type (e.g. person, project).
- `--status <STATUS>`: Only this state: detected, established, or archived.
- `--query <QUERY>`: Only entities whose name or alias contains this text.
- `-l, --limit <LIMIT>`: Max entities to return. Default: `50`.
- `--offset <OFFSET>`: Skip this many entities first. Default: `0`.

#### `wenlan entities archive`

Archive entities by id. They stay stored and can be restored.

```text
wenlan entities archive [OPTIONS] <IDS>...
```

Arguments:

- `<IDS>...`: Entity ids to archive.

Options:

- `--dry-run`: Show what would be archived without changing anything.

#### `wenlan entities restore`

Restore archived entities by id.

```text
wenlan entities restore [OPTIONS] <IDS>...
```

Arguments:

- `<IDS>...`: Entity ids to restore.

Options:

- `--dry-run`: Show what would be restored without changing anything.

#### `wenlan entities merge`

Merge one entity into another. The loser is deleted; its observations, memory links, edges, and aliases move to the canonical.

```text
wenlan entities merge [OPTIONS] --into <INTO> <LOSER>
```

Arguments:

- `<LOSER>`: The entity to merge away (id or exact name) — the loser.

Options:

- `--into <INTO>`: The entity to merge into (id or exact name) — the canonical.
- `--dry-run`: Preview the merge counts without mutating anything.

#### `wenlan entities alias`

Declare an additional name for an entity.

```text
wenlan entities alias [OPTIONS] <ENTITY> <ALIAS>
```

Arguments:

- `<ENTITY>`: Entity id or exact name.
- `<ALIAS>`: The alias to add.

<!-- cli-reference:end -->

## Output formats

- `--format auto` (default): table on TTY, JSON when piped.
- `--format json`: pretty-printed JSON.
- `--format table`: human-readable table.
- `--quiet` / `-q`: suppress success output (errors still go to stderr).

## Links

- [wenlan.app](https://wenlan.app) — project home
- [wenlan.app/docs/get-started](https://wenlan.app/docs/get-started) — install + verify the first memory loop
- [wenlan.app/docs/commands](https://wenlan.app/docs/commands) — Claude Code commands and MCP tools reference
- [wenlan.app/docs/troubleshooting](https://wenlan.app/docs/troubleshooting) — common failure modes
- [github.com/wenlan-app/wenlan](https://github.com/wenlan-app/wenlan) — source
- [github.com/wenlan-app/wenlan/releases/latest](https://github.com/wenlan-app/wenlan/releases/latest) — desktop app and runtime downloads

## License

Apache-2.0. See top-level LICENSE.
