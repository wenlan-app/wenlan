# wenlan-cli

Wenlan's command line. Wenlan holds the docs, skills and rules your AI tools keep current and work from; the CLI is how you capture, recall, hand off, and keep the pages healthy from a terminal, plus set up and check the local runtime.

License: Apache-2.0.

## Install

One line on macOS Apple Silicon, Linux x64 or ARM64 (glibc), and Windows x64. It needs Node.js 18 or newer:

```bash
npx -y wenlan setup
```

It downloads `wenlan`, `wenlan-server`, and `wenlan-mcp` for this computer into `~/.wenlan/bin/` (`%USERPROFILE%\.wenlan\bin` on Windows, with the DLLs the daemon loads), checks the download against the release's `SHA256SUMS`, configures local memory, starts the background runtime, and prints its status. After that, `npx wenlan <command>` runs the installed CLI; the last lines of setup say how to put `wenlan` itself on `PATH`.

On Linux with no systemd user session (a container, WSL without systemd, some SSH logins) nothing can keep Wenlan running in the background. Setup starts it for now and says how to start it again after a restart.

Without Node.js on macOS or Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash
export PATH="$HOME/.wenlan/bin:$PATH"
wenlan setup --basic
wenlan start
wenlan status
```

Without Node.js on Windows, unpack `wenlan-windows-x64.zip` from [Releases](https://github.com/wenlan-app/wenlan/releases/latest) as a unit into a directory on `PATH`, then run the same three `wenlan` commands.

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

Browse distilled pages, or open one in your editor by title query. The pages come from the folder of the daemon the CLI talks to, so `WENLAN_HOST` selects whose pages are listed. A daemon selected with `WENLAN_HOST` that is unreachable is an error like in the other read commands; while the default local daemon is stopped, the folder from the local config is listed.

```bash
wenlan pages
wenlan pages "project roadmap"
wenlan pages "project roadmap" --print
```

`--print` writes the page text and the memories it was written from to the terminal instead of opening it. If the query matches more than one page, it lists them and asks you to narrow the query. With `--format json` it prints one object with `page` and `sources`.

**Change pages from the terminal**

Everything an AI tool can do to a page through the MCP server, under the same names. A page is named by its title, part of its title, or its id; if more than one page matches, the command lists them and stops.

```bash
wenlan pages read "project roadmap"
wenlan pages write --title "Project roadmap" --file roadmap.md
wenlan pages write "project roadmap" --file roadmap.md   # replace the text
wenlan pages rename "project roadmap" "Roadmap 2027"
wenlan pages move "Roadmap 2027" plans/2027               # folders are created as needed
wenlan pages history "Roadmap 2027"
wenlan pages restore "Roadmap 2027" <version>
wenlan pages links "Roadmap 2027"
```

These commands act as an AI tool does, so the same limits apply. `wenlan pages delete` cannot remove a page a person wrote; delete those in the app.

**`wenlan memories pending | confirm | forget | history`**

```bash
wenlan memories pending
wenlan memories confirm <id>
wenlan memories forget <id>
```

**`wenlan tool`**

The full tool list the MCP server offers, callable by name. Use it for anything that has no command of its own yet.

```bash
wenlan tool list
wenlan tool show write_page
wenlan tool call list_pages limit=5
wenlan tool call write_page --json '{"title":"Notes","content":"Hello"}'
```

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

**`wenlan init`**

Run it once inside a git repository. It makes a Space named after the repository, ties the two, and offers to connect the AI tools it finds. The first time, it also asks whether to open a pull request every day (Enter means yes; `wenlan repo bind --daily off` stops it). After that the terminal and AI tools started in that folder use that Space; `wenlan status` shows which one.

```bash
cd ~/Repos/acme-api
wenlan init
wenlan status
```

**`wenlan spaces`**

Manage Spaces by hand. `wenlan init` covers the usual case.

```bash
wenlan spaces list
wenlan spaces add ideas --default
wenlan spaces show career
wenlan spaces default work
wenlan spaces move scratch career
```

**`wenlan team`**

Run a team server (add members, share Spaces) or join one with a join code.

```bash
wenlan team status
wenlan team add Ana --access Work --access Notes:read --url https://wenlan.example.com
wenlan team join "https://wenlan.example.com#wlt_…" --server-space Work
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
wenlan setup --model qwen3.5-4b # download/select a local model
wenlan setup --anthropic-api-key-env ANTHROPIC_API_KEY
```

**`wenlan background <on|off>` and `wenlan restart`**

Start or stop the per-user background runtime. `wenlan stop` stops the current daemon while preserving its login registration and writes `<data root>/autostart.off`, so recovery will not start it again. Run `wenlan start` to remove the marker and start it again; it waits up to 10 s for the daemon to answer before saying "started" and, on macOS, reports an error the daemon logged during this start (never one left over from an earlier run). Most users run `wenlan start` once after setup and `wenlan restart` after upgrades installed outside `wenlan setup`.

```bash
wenlan start
wenlan stop
wenlan restart
```

**`wenlan steep <status|configure|disable|run>`**

Models and provider keys are capability only; they do not authorize automatic
model calls. Inspect the two job groups, confirm exact hard-pinned sources, or
turn model-backed background work off while keeping providers installed:

```bash
wenlan steep status
wenlan steep configure --everyday on-device --synthesis on-device
wenlan steep disable
wenlan steep run
```

`run` does one full pass now instead of waiting for the background: it
organizes what came in and writes new pages from it with the page model chosen
in `configure`, then says how many pages it made. With no page model chosen it
writes nothing and says so.

`configure` shows what each job does and discloses cloud cost/data transfer or
on-device CPU/GPU/RAM use before confirmation. An unavailable hard pin pauses;
Wenlan never substitutes another provider. Deterministic storage, indexing, and
sync continue when Steep is off.

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
wenlan models install qwen3.5-4b
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
wenlan connect cursor --all-tools
```

The client is given the 23 everyday tools: reading and searching, the approved working rules and saving one the person asks for, the skills Wenlan holds and the search for more, saving, writing and reshaping pages, and writing pages on demand. A client loads every listed tool into each conversation, and the full list of 63 is more than twice the size. `--all-tools` lists them all (health checks and repairs, review queues, entities, folders, version history); without it they are still one `wenlan tool call <name>` away in a terminal.

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
- `--session <TOOL:ID>`: AI session as tool:id (for example claude-code:<session id>); links what you write to that conversation. Defaults to WENLAN_SESSION, then CODEX_THREAD_ID.
- `--space <SPACE>`: Use one registered Space for scope-aware reads and writes.
- `--all-spaces`: Read across every Space. Invalid for writes and strict WENLAN_SPACE pins.

**Start**

### `wenlan setup`

Set Wenlan up and connect the AI tools on this computer; optionally a local model or an Anthropic key.

```text
wenlan setup [OPTIONS]
```

Options:

- `--basic`: Set up without a model or API key.
- `--model <MODEL_ID>`: Download and select a local model, for example qwen3.5-4b.
- `--anthropic-api-key-env <ENV_VAR>`: Read an Anthropic key from this environment variable.
- `-y, --yes`: Skip confirmation prompts where possible.
- `--config-only`: Write the settings only: connect no AI tool and ask nothing.

### `wenlan init`

Set this repository up: a Space named after it, tied to it, and your AI tools connected.

```text
wenlan init [OPTIONS]
```

### `wenlan open`

Open Wenlan's personal browser UI.

```text
wenlan open [OPTIONS]
```

### `wenlan status`

Check whether Wenlan is running, and which Space this folder uses.

```text
wenlan status [OPTIONS]
```

**Daily**

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

Find the memories and pages that match a query.

```text
wenlan recall [OPTIONS] <QUERY>
```

Arguments:

- `<QUERY>`: Query to recall memories for.

Options:

- `-l, --limit <LIMIT>`: Max memories to return. Default: `10`.

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

### `wenlan handoff`

Hand off work through the Space Brief; keeps a project file if Wenlan is down.

```text
wenlan handoff [OPTIONS] --file <JSON_FILE>
       wenlan handoff [OPTIONS] <COMMAND>
```

Options:

- `--file <JSON_FILE>`: Read a HandoffRequest or wenlan.handoff.v1 file.

#### `wenlan handoff import-pending`

Import pending handoff files from this project's root.

```text
wenlan handoff import-pending [OPTIONS]
```

### `wenlan rules`

Read and save working rules for a Space, or manage your personal AI-tool setup.

```text
wenlan rules [OPTIONS] [COMMAND]
```

#### `wenlan rules add`

Save a rule your AI tools follow from now on (what an AI tool does with add_rule).

```text
wenlan rules add [OPTIONS] <TEXT>
```

Arguments:

- `<TEXT>`: The rule, as one sentence.

Options:

- `--every-space`: Save it for every Space instead of this one. Not with --space.
- `--kind <KIND>`: Save it as a decision or a pitfall instead of a rule. Possible values: `rule`, `decision`, `pitfall`.

#### `wenlan rules remove`

Remove a rule saved this way, by the id `wenlan rules` prints (remove_rule).

```text
wenlan rules remove [OPTIONS] <ID>
```

Arguments:

- `<ID>`: The rule's id, as `wenlan rules` prints it.

#### `wenlan rules adopt`

Adopt personal rules, skills, commands, subagents and settings.

```text
wenlan rules adopt [OPTIONS]
```

Options:

- `--dry-run`: Preview the changes, or list the installed files while offline.

#### `wenlan rules restore`

Restore the original files and links from the local setup backup.

```text
wenlan rules restore [OPTIONS]
```

#### `wenlan rules sync`

Merge personal settings and check the local tool setup for changes to review.

```text
wenlan rules sync [OPTIONS]
```

#### `wenlan rules skill-updates`

Check adopted skills for upstream releases and list offers for review.

```text
wenlan rules skill-updates [OPTIONS]
```

#### `wenlan rules settings`

Preview and approve an explicit local rulesync settings source.

```text
wenlan rules settings [OPTIONS] <SOURCE>
```

Arguments:

- `<SOURCE>`: A local rulesync source folder.

Options:

- `--dry-run`: Show only the proposed settings changes.

#### `wenlan rules review`

Answer a proposal shown by `wenlan rules sync`.

```text
wenlan rules review [OPTIONS] <ID> <ACTION>
```

Arguments:

- `<ID>`: The proposal ID shown by `wenlan rules sync`.
- `<ACTION>` Possible values: `tool-only` (Keep the change for the tool that made it), `all-tools` (Share the change with all compatible tools), `dismiss` (Discard the proposal).

Options:

- `--file <PATH>`: Choose the source file when a proposal contains competing edits.

### `wenlan pages`

List pages or open one by title; read, write, rename, move, delete or restore one (read, write, rename, delete, move, history, restore, links, lock, unlock).

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

#### `wenlan pages read`

Print a page: its title, text and sources (the read_page tool).

```text
wenlan pages read [OPTIONS] <PAGE>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.

#### `wenlan pages write`

Create a page, or replace the text of an existing one (the write_page tool). Text comes from --file or stdin.

```text
wenlan pages write [OPTIONS] [PAGE]
```

Arguments:

- `[PAGE]`: An existing page to replace: its title, part of it, or its id. Leave out to create a page.

Options:

- `--title <TITLE>`: Title for a new page.
- `--file <PATH>`: Read the text from this file instead of stdin.
- `--summary <SUMMARY>`: One-sentence summary.

#### `wenlan pages rename`

Change a page's title; links to it follow (the rename_page tool).

```text
wenlan pages rename [OPTIONS] <PAGE> <TITLE>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.
- `<TITLE>`: The new title.

#### `wenlan pages delete`

Delete a page (the delete_page tool). Asks first.

```text
wenlan pages delete [OPTIONS] <PAGE>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.

Options:

- `-y, --yes`: Do not ask.

#### `wenlan pages move`

Move a page into a folder; "" is the top level (the move_page tool).

```text
wenlan pages move [OPTIONS] <PAGE> <FOLDER>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.
- `<FOLDER>`: Destination folder path.

#### `wenlan pages history`

List a page's versions, newest first (the get_page_revisions tool).

```text
wenlan pages history [OPTIONS] <PAGE>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.

#### `wenlan pages restore`

Put an earlier version of a page back (the restore_page_version tool).

```text
wenlan pages restore [OPTIONS] <PAGE> <VERSION>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.
- `<VERSION>`: The version to put back, as `wenlan pages history` lists it.

#### `wenlan pages links`

Show the pages this page links to and the pages linking to it (the get_page_links tool).

```text
wenlan pages links [OPTIONS] <PAGE>
```

Arguments:

- `<PAGE>`: The page's title, part of it, or its id.

**Team**

### `wenlan team`

Work with a team: start one here, invite people, or join with an invite link (start, share, invite, join, status, leave, update, remove, new-key, stop).

```text
wenlan team [OPTIONS] <COMMAND>
```

#### `wenlan team draft`

Prepare a private team publication draft from your personal library.

```text
wenlan team draft [OPTIONS] <KIND> <ID>
```

Arguments:

- `<KIND>` Possible values: `page`, `memory`.
- `<ID>`

#### `wenlan team publish`

Review and publish a personal item from the team host's terminal.

```text
wenlan team publish [OPTIONS] <KIND> <ID>
```

Arguments:

- `<KIND>` Possible values: `page`, `memory`.
- `<ID>`: ID in this computer's personal library.

Options:

- `--space-id <SPACE_ID>`: Destination Space ID. Omit to publish directly to the team.
- `--brief`: Publish the page as the Brief for this destination.
- `--text <TEXT>`: A local JSON file containing edited title and body.
- `--redactions <REDACTIONS>`: A local JSON object mapping personal source IDs to edited title and body. Sources still require selection during review.

#### `wenlan team create-space`

Create a group for team pages and memories without sharing a personal Space.

```text
wenlan team create-space [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`

Options:

- `--restricted`: Require a grant for this Space.

#### `wenlan team grant`

Set a member's team-wide library grant. Their Space grants are separate.

```text
wenlan team grant [OPTIONS] <MEMBER> <ACCESS>
```

Arguments:

- `<MEMBER>`
- `<ACCESS>` Possible values: `read`, `write`, `none`.

#### `wenlan team start`

Start a team on this computer: others reach it with an invite link.

```text
wenlan team start [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The team's name, such as the company's. Everyone on it sees it.

Options:

- `--as <YOUR NAME>`: Your own name, as the team sees what you write. Left out, the name you already have in Wenlan.
- `--url <URL>`: The address others reach this computer at, put in every invite link. Left out, Wenlan picks one: its Tailscale address first, else its address on the local network. A loopback address (`http://127.0.0.1:<PORT>`, `localhost`) keeps the team on this computer only: nothing else on the network can reach it.
- `--port <PORT>`: The port the team listens on. Left out, 7879.

#### `wenlan team stop`

Stop letting the team reach this computer. The team, its people and Spaces stay; `wenlan team start` turns it on again.

```text
wenlan team stop [OPTIONS]
```

#### `wenlan team status`

Show the team this computer runs (its people and Spaces) and the teams it has joined.

```text
wenlan team status [OPTIONS]
```

#### `wenlan team rename`

Name the team; every member's Wenlan shows the name.

```text
wenlan team rename [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The team's name, such as the company's.

#### `wenlan team invite`

Invite someone: add them and print their invite link once.

```text
wenlan team invite [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The person's name.

Options:

- `--access <SPACE[:read]>`: A Space they can use, `Space` for read and write or `Space:read` for read only. Repeat for more Spaces. Left out, they get the Spaces open to everyone on the team.
- `--url <URL>`: The address their computer reaches this one at, put in the invite link. Left out, the address the team was started with.

#### `wenlan team update`

Rename a member or replace the Spaces they can use.

```text
wenlan team update [OPTIONS] <MEMBER>
```

Arguments:

- `<MEMBER>`: The member's name, or their id from `wenlan team status --format json`.

Options:

- `--name <NAME>`: The new name.
- `--access <SPACE[:read]>`: A Space the member can use, `Space` or `Space:read`. Repeat for more; the list replaces the current one.

#### `wenlan team remove`

Remove a member: their key stops working at once.

```text
wenlan team remove [OPTIONS] <MEMBER>
```

Arguments:

- `<MEMBER>`: The member's name, or their id from `wenlan team status --format json`.

#### `wenlan team new-key`

Give a member a new invite link; the old one stops working.

```text
wenlan team new-key [OPTIONS] <MEMBER>
```

Arguments:

- `<MEMBER>`: The member's name, or their id from `wenlan team status --format json`.

Options:

- `--url <URL>`: The address the member's computer reaches this one at, put in the invite link. Left out, the address the team was started with.

#### `wenlan team share`

Change access to an existing hosted Space.

```text
wenlan team share [OPTIONS] <SPACE>
```

Arguments:

- `<SPACE>`: Existing hosted Space name.

Options:

- `--everyone <ACCESS>`: Open it to everyone on the team, now and when invited later: `--everyone` to read and write, `--everyone read` to read only, `--everyone nobody` to take that back. Left out, only the people given it with `--access` can use it.

#### `wenlan team join`

Join a team with an invite link: every Space open to you is synced to this computer, and Spaces opened to you later follow.

```text
wenlan team join [OPTIONS] <INVITE LINK>
```

Arguments:

- `<INVITE LINK>`: The invite link you were sent (`https://…/join#wlt_…`).

Options:

- `--server-space <SERVER_SPACE>`: Take only this Space of the team (not `--space`, which picks a Space on this computer).
- `--as <LOCAL_SPACE>`: The Space's name on this computer. Left out, the team's name for it.
- `--list`: Only show the Spaces the link opens; join nothing.

#### `wenlan team leave`

Leave a team you joined: its Spaces stop syncing here. The pages already on this computer stay.

```text
wenlan team leave [OPTIONS] [TEAM]
```

Arguments:

- `[TEAM]`: The team's name or its server's address. Not needed when this computer is on one team.

#### `wenlan team keys`

List the AI tools you connected to a team with `wenlan login`.

```text
wenlan team keys [OPTIONS]
```

Options:

- `--team <TEAM>`: The team's name or its server's address. Left out, every team this computer joined.

#### `wenlan team revoke-key`

Remove an AI tool you connected: it stops working at its next request.

```text
wenlan team revoke-key [OPTIONS] <ID>
```

Arguments:

- `<ID>`: The tool's id, from `wenlan team keys`.

Options:

- `--team <TEAM>`: The team's name or its server's address. Not needed when this computer is on one team.

### `wenlan login`

Connect an AI tool on this computer to a team with a short code you allow in a browser.

```text
wenlan login [OPTIONS] --tool <TOOL> <SERVER>
```

Arguments:

- `<SERVER>`: The team's address, such as `https://team.example.com`.

Options:

- `--tool <TOOL>`: The AI tool to connect: `claude-code`, `codex` or `cursor`.

**Running**

### `wenlan connect`

Connect Wenlan to a supported agent or editor.

```text
wenlan connect [OPTIONS] <CLIENT>
```

Arguments:

- `<CLIENT>`: Client to configure. Possible values: `claude-code` (Claude Code without the Wenlan plugin), `codex` (OpenAI Codex CLI), `gemini` (Gemini CLI), `cursor` (Cursor editor), `claude-desktop` (Claude Desktop), `vscode` (VS Code workspace MCP config).

Options:

- `--dry-run`: Print the command or file edit without changing anything.
- `--all-tools`: List every Wenlan tool in the client, not only the everyday ones. Health checks, review queues, entities, folders and version history then load into each conversation too.

### `wenlan doctor`

Diagnose runtime, model, and API key setup.

```text
wenlan doctor [OPTIONS]
```

Options:

- `--repair`: Point older ~/.wenlan/bin copies at the app bundle. The previous file is renamed aside, not deleted.

### `wenlan start`

Start Wenlan now and keep it running in the background after login.

```text
wenlan start [OPTIONS]
```

### `wenlan stop`

Stop Wenlan. It stays off until `wenlan start`.

```text
wenlan stop [OPTIONS]
```

### `wenlan restart`

Restart the Wenlan background process. Required after an update.

```text
wenlan restart [OPTIONS]
```

**More**

### `wenlan skills`

Find, save, review and install skills your AI tools can use.

```text
wenlan skills [OPTIONS] [COMMAND]
```

#### `wenlan skills list`

List the skills Wenlan holds (what an AI tool gets from list_skills).

```text
wenlan skills list [OPTIONS]
```

#### `wenlan skills search`

Search skills.sh (what an AI tool gets from search_skills).

```text
wenlan skills search [OPTIONS] <QUERY>...
```

Arguments:

- `<QUERY>...`: What your AI tools should be able to do.

Options:

- `--warnings`: Also show skills whose safety checks warned or have not run.

#### `wenlan skills show`

Show everything in one skill and its safety checks, before installing.

```text
wenlan skills show [OPTIONS] <SKILL>
```

Arguments:

- `<SKILL>`: The skill as search prints it: owner/repo/skill.

#### `wenlan skills scan`

List skills already in your AI tools' folders that Wenlan does not hold.

```text
wenlan skills scan [OPTIONS]
```

#### `wenlan skills import`

Take skills from your AI tools' folders into Wenlan. Asks you first.

```text
wenlan skills import [OPTIONS] [NAMES]...
```

Arguments:

- `[NAMES]...`: Names from `wenlan skills scan`. Omit to take all of them.

#### `wenlan skills install`

Install a skill from skills.sh. Shows it and asks you first.

```text
wenlan skills install [OPTIONS] <SKILL>
```

Arguments:

- `<SKILL>`: The skill as search prints it: owner/repo/skill.

Options:

- `--only-in <SPACE>`: Use it only while working in this Space instead of everywhere.

#### `wenlan skills remove`

Remove a skill from Wenlan and from the AI tools it was copied to. Asks you first.

```text
wenlan skills remove [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The skill's name as `wenlan skills list` prints it.

#### `wenlan skills save`

Save your own skill from a Markdown body and optional text files.

```text
wenlan skills save [OPTIONS] --description <DESCRIPTION> --body <FILE> <NAME>
```

Arguments:

- `<NAME>`: Folder name for the skill.

Options:

- `--description <DESCRIPTION>`: When an AI tool should use the skill.
- `--body <FILE>`: Markdown file containing the SKILL.md body without YAML front matter.
- `--extra <PATH>`: A UTF-8 file relative to the body file's folder. Repeat for each extra file.
- `--tool <TOOL>`: Use the skill for one tool. Omit to share across tools. Possible values: `claude-code`, `codex`, `gemini-cli`, `antigravity`, `copilot-cli`, `cursor`, `opencode`, `muse-code`.

#### `wenlan skills pending`

List personal skill proposals waiting for your review.

```text
wenlan skills pending [OPTIONS]
```

#### `wenlan skills review`

Show a skill proposal and ask whether to accept or dismiss it.

```text
wenlan skills review [OPTIONS] <ID> <ACTION>
```

Arguments:

- `<ID>`: A proposal ID printed by `wenlan skills pending`.
- `<ACTION>` Possible values: `accept`, `dismiss`.

Options:

- `--tool <TOOL>`: Keep an accepted skill for one tool. Possible values: `claude-code`, `codex`, `gemini-cli`, `antigravity`, `copilot-cli`, `cursor`, `opencode`, `muse-code`.
- `--shared`: Share an accepted skill across tools.

### `wenlan memories`

List recent memories, or confirm, forget and review them (pending, confirm, forget, history).

```text
wenlan memories [OPTIONS]
       wenlan memories <COMMAND>
```

Options:

- `-l, --limit <LIMIT>`: Max results. Default: `20`.
- `-t, --type <MEMORY_TYPE>`: Filter by memory type.
- `--pending`: Only pending captures (not yet confirmed; what the MCP list_pending tool returns).

#### `wenlan memories pending`

List captures waiting for review (the list_pending tool).

```text
wenlan memories pending [OPTIONS]
```

Options:

- `-l, --limit <LIMIT>`: Max results. Default: `20`.

#### `wenlan memories confirm`

Confirm a pending memory (the confirm_memory tool).

```text
wenlan memories confirm [OPTIONS] <ID>
```

Arguments:

- `<ID>`: The memory's id, as `wenlan memories` prints it.

#### `wenlan memories forget`

Delete a memory (the forget tool). Asks first.

```text
wenlan memories forget [OPTIONS] <ID>
```

Arguments:

- `<ID>`: The memory's id, as `wenlan memories` prints it.

Options:

- `-y, --yes`: Do not ask.

#### `wenlan memories history`

Show how a memory changed over time (the get_memory_revisions tool).

```text
wenlan memories history [OPTIONS] <ID>
```

Arguments:

- `<ID>`: The memory's id.

### `wenlan sources`

Manage folders and files Wenlan should learn from (add, save, read).

```text
wenlan sources [OPTIONS] <COMMAND>
```

#### `wenlan sources read`

Print the text of one saved file or webpage (the read_source tool). Needs --space.

```text
wenlan sources read [OPTIONS] <SOURCE_ID>
```

Arguments:

- `<SOURCE_ID>`: The source's id.

Options:

- `--kind <KIND>`: What the source is. Possible values: `file`, `webpage`. Default: `file`.
- `--offset <OFFSET>`: Start at this character. Default: `0`.
- `--limit <LIMIT>`: Max characters to print (1 to 16000). Default: `8000`.

#### `wenlan sources add`

Add a folder or file source and sync it now.

```text
wenlan sources add [OPTIONS] <PATH>
```

Arguments:

- `<PATH>`: Path to a directory or file to add as a source.

Options:

- `--type <SOURCE_TYPE>`: Kind of source. `okf` imports an OKF bundle (for example an OpenWiki folder). Pass --space to put the pages in one Space. Possible values: `directory` (A folder or single file of documents), `okf` (An OKF knowledge bundle). Default: `directory`.

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

### `wenlan follow`

Make a page or a repo file follow a page, so it keeps up when that page changes.

```text
wenlan follow [OPTIONS] <WHAT> <PAGE>
```

Arguments:

- `<WHAT>`: What follows: a page (its title, part of it, or its id) or a file of this repository such as README.md.
- `<PAGE>`: The page it follows: its title, part of it, or its id.

### `wenlan unfollow`

Stop a page or a repo file following a page.

```text
wenlan unfollow [OPTIONS] <WHAT> [PAGE]
```

Arguments:

- `<WHAT>`: The page or file that stops following.
- `[PAGE]`: The page it follows. Needed only when it follows more than one.

### `wenlan follows`

Show what follows which page, and what is waiting to catch up.

```text
wenlan follows [OPTIONS] [PAGE]
```

Arguments:

- `[PAGE]`: Show what this page follows. Leave out to list the files that follow a page and the pages waiting to catch up.

### `wenlan spaces`

Manage Spaces, the separate sets of pages Wenlan keeps (list, add, default, show).

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

#### `wenlan spaces show`

Show detail for a space — name, description, memory count, entity count, default + starred flags.

```text
wenlan spaces show [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: Space name.

### `wenlan repo`

Put this Space's code pages into its git repository as a pull request. `wenlan wiki` is the earlier name and still works.

```text
wenlan repo [OPTIONS] <COMMAND>
```

#### `wenlan repo bind`

Tie the Space to this git repository (or --repo), so its code pages can go into the repository as a pull request.

```text
wenlan repo bind [OPTIONS]
```

Options:

- `--repo <REPO>`: Any folder inside the repository. Default: the current folder.
- `--folder <FOLDER>`: Folder inside the repository for the pages. Default: wiki.
- `--daily <HOUR|off>`: Bring the wiki up to date by itself every day after this local hour (0-23), or `off`.
- `--agent <AGENT>`: The AI tool the daily run starts. Default: claude. Possible values: `claude`, `codex`, `cursor`, `copilot`.
- `--on-github`: Run the daily update on GitHub Actions instead of this computer: the next publish adds the workflow to the pull request.
- `--confirm-repo <OWNER/NAME>`: The repository pull requests go to (`owner/name` on GitHub, else the remote URL). Needed when it is not yours and GitHub does not let you push to it.
- `--take-over`: Publish from this computer although another Wenlan (a teammate's, or another computer of yours) keeps this repository's pages and rules. The next publish replaces theirs.

#### `wenlan repo publish`

Write the Space's code pages into the repository and open or update the pull request.

```text
wenlan repo publish [OPTIONS]
```

#### `wenlan repo status`

Show the repository the Space is bound to and the last publish.

```text
wenlan repo status [OPTIONS]
```

#### `wenlan repo unbind`

Stop putting this Space's pages into its repository.

```text
wenlan repo unbind [OPTIONS]
```

#### `wenlan repo rules-proposals`

Show repository rule changes waiting for review in this Space.

```text
wenlan repo rules-proposals [OPTIONS]
```

#### `wenlan repo rules-review`

Review a repository rule change and decide whether to keep it in this Space.

```text
wenlan repo rules-review [OPTIONS] <ID> <ACTION>
```

Arguments:

- `<ID>`: The proposal ID from `wenlan wiki rules-proposals`.
- `<ACTION>` Possible values: `accept`, `dismiss`.

#### `wenlan repo rules-checkout`

Prepare readable rules and project skills for this checkout in Wenlan's data directory.

```text
wenlan repo rules-checkout [OPTIONS]
```

Options:

- `--repo <REPO>`: A directory inside the checkout. Defaults to the current directory.

#### `wenlan repo rules-team`

Include one team Rules layer in future wiki pull requests, or turn it off.

```text
wenlan repo rules-team [OPTIONS] <TEAM>
```

Arguments:

- `<TEAM>`: The team's name or ID, or `off`.

#### `wenlan repo shared-rules-propose`

Propose the next Rules page for a Space hosted on this computer.

```text
wenlan repo shared-rules-propose [OPTIONS] <FILE>
```

Arguments:

- `<FILE>`

#### `wenlan repo shared-rules-proposals`

List Rules proposals for a Space hosted on this computer.

```text
wenlan repo shared-rules-proposals [OPTIONS]
```

#### `wenlan repo shared-rules-review`

Review a Rules proposal in a Space hosted on this computer.

```text
wenlan repo shared-rules-review [OPTIONS] <ID> <ACTION>
```

Arguments:

- `<ID>`
- `<ACTION>` Possible values: `accept`, `dismiss`.

#### `wenlan repo refresh`

Check the wiki folder of this checkout against the code: fix line numbers that moved and have an AI tool rewrite pages whose code changed. Works without Wenlan running; the GitHub Actions workflow runs it.

```text
wenlan repo refresh [OPTIONS]
```

Options:

- `--repo <REPO>`: Any folder inside the repository. Default: the current folder.
- `--folder <FOLDER>`: The folder of pages inside the repository. Default: wenlan, or wiki when the repository already holds its pages there.
- `--agent <AGENT>`: The AI tool that rewrites pages whose code changed. Possible values: `claude`, `codex`, `cursor`, `copilot`, `none` (Only fix line numbers and list the pages that need rewriting). Default: `claude`.
- `--pull-request`: Commit the changed pages on `wenlan/wiki-refresh` and open or update the pull request.

#### `wenlan repo workflow`

Print, or `--write`, the GitHub Actions workflow that runs `refresh` every day and opens a pull request when a page changed.

```text
wenlan repo workflow [OPTIONS]
```

Options:

- `--repo <REPO>`: Any folder inside the repository. Default: the current folder.
- `--folder <FOLDER>`: The folder of pages inside the repository. Default: wenlan, or wiki when the repository already holds its pages there.
- `--agent <AGENT>`: The AI tool the workflow runs. Possible values: `claude`, `codex`, `cursor`, `copilot`. Default: `claude`.
- `--utc-hour <UTC_HOUR>`: The hour (0-23, UTC) it runs every day. Default: `1`.
- `--write`: Write it into the repository's GitHub workflows folder, as wenlan-wiki.yml, instead of printing it.

### `wenlan curate`

List changes waiting for review, or dismiss one. Accepting is done in the Wenlan app.

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

- `[MODEL_ID]`: Model id, for example qwen3.5-4b.

Options:

- `-y, --yes`: Skip confirmation before downloading.

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

#### `wenlan steep run`

Steep once now: organize what came in and write new pages from it.

```text
wenlan steep run [OPTIONS]
```

### `wenlan outbox`

Show or send what was saved locally while Wenlan was not running.

```text
wenlan outbox [OPTIONS] <COMMAND>
```

#### `wenlan outbox status`

List what is waiting or failed, without contacting Wenlan.

```text
wenlan outbox status [OPTIONS]
```

#### `wenlan outbox drain`

Send what is waiting to Wenlan now.

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

### `wenlan tool`

Call any Wenlan tool an AI tool can call, by the same name (list, show, call).

```text
wenlan tool [OPTIONS] <COMMAND>
```

#### `wenlan tool list`

List every tool, with one line on what it does.

```text
wenlan tool list [OPTIONS]
```

#### `wenlan tool show`

Show one tool's description and the arguments it takes.

```text
wenlan tool show [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: Tool name, as `wenlan tool list` prints it.

#### `wenlan tool call`

Call a tool. Arguments are `key=value` pairs, or one JSON object with --json.

```text
wenlan tool call [OPTIONS] <NAME> [ARGS]...
```

Arguments:

- `<NAME>`: Tool name, as `wenlan tool list` prints it.
- `[ARGS]...`: Arguments as key=value. A value that parses as JSON (a number, true, a list) is sent as that; anything else is sent as text. `key=@file` reads the value from a file, `key=-` from stdin.

Options:

- `--json <OBJECT>`: All arguments as one JSON object. Not with key=value pairs.

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
