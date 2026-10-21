# wenlan-mcp

MCP server for [Wenlan](https://github.com/wenlan-app/wenlan). It lets Claude Code, Cursor, Codex, Claude Desktop, Gemini CLI, and other MCP clients read and write to the local Wenlan daemon through the [Model Context Protocol](https://modelcontextprotocol.io).

Wenlan owns storage, search, embeddings, pages, and distill cycles. `wenlan-mcp` is the connector.

## Install

Most users should install the runtime through the root README (`npx -y wenlan setup` on macOS Apple Silicon, the automated shell setup on Linux, or the matching release archive on Windows). Then use the product CLI to configure supported clients:

```bash
wenlan connect codex              # or: claude-code, cursor, claude-desktop, vscode, gemini
wenlan connect cursor --dry-run   # preview before editing JSON config
```

MCP-only setup gives agents the whole tool surface: capture and recall, reading and writing wiki pages, the Space-owned Brief with `update_brief` for handoff, and page distillation. It does not install the Claude Code slash skills (`/wenlan`, `/brief`, `/handoff`), which wrap those same tools in a guided flow; install the Wenlan plugin if you want them.

If you only need the raw MCP connector config, add this to your MCP client:

```json
{
  "mcpServers": {
    "wenlan": {
      "command": "npx",
      "args": ["-y", "wenlan-mcp"]
    }
  }
}
```

The npm wrapper auto-detects the host platform and downloads the matching prebuilt binary from the Wenlan release. Supported: macOS (arm64), Linux (x64, arm64; glibc), Windows (x64). Other targets require building the connector from source via `cargo install --locked wenlan-mcp`; macOS Intel does not currently have a supported complete local runtime.

Or install a binary directly:

```bash
brew install 7xuanlu/tap/wenlan-mcp
cargo install --locked wenlan-mcp
```

`--locked` builds with the dependency versions the release was tested with; a plain `cargo install wenlan-mcp` also works, since the crate pins its proc-macro crate to the same minor as `rmcp`.

Then use:

```json
{
  "mcpServers": {
    "wenlan": {
      "command": "wenlan-mcp"
    }
  }
}
```

`wenlan-mcp` expects the Wenlan daemon at `http://127.0.0.1:7878` by default. Override it with:

```bash
wenlan-mcp --origin-url http://127.0.0.1:7879
```

## Tools

The local stdio surface is locked at exactly 33 tools. The table below calls
out the wiki loop, the primary memory loop, and the unique refinement-review
queue.

| Tool | Purpose |
| --- | --- |
| `list_pages` | List wiki pages (id, title, summary, Space, status, last modified, creation kind, source count, review lock) to browse what exists. |
| `read_page` | Read one wiki page in full, including its version and review lock, together with the source memories it cites, the notes it relies on (`basis`), the code it relies on (`code_basis`), and plain links that changed (`linked_changed`). |
| `write_page` | Create or update a wiki page; sources are optional. Pass `expected_version` from `read_page` on a refresh; a stale version is refused, and a locked page or an unversioned refresh of an edited page goes through a revision card. Pass `basis` (`{page_id, version}` per note read) when the page was written from other notes; the reply lists the page's bases. Pass `code_basis` (`{repo_root, path, line_start, line_end}`) for lines of code the page relies on; Wenlan reads the file when you write and tells you when the lines change. |
| `page_basis` | Mark, diff, confirm or remove what one page relies on another page for. `mark` and `confirm` take the basis note's `version` and are refused if it moved on. Changes are local stdio only. |
| `page_code_basis` | Confirm or remove a line span of code a page relies on. `confirm` re-reads the file and re-anchors the row as `ok` after you have looked at the change. Pass the row's `repo_root`, `path`, `line_start` and `line_end` as `read_page` shows them. Local stdio only. |
| `brief` | Read the current Space Brief; an optional topic appends separately labeled same-Space context. |
| `update_brief` | Hand off to the next session: replace the summary and add, edit, move, gate, or complete items. Local stdio only. |
| `capture` | Save one durable memory and return its explicit `source_memory_id`. |
| `save_source` | Save a document read in another app or on the web (email, Notion or Drive page, Slack thread, article) with its link, so notes cite the original. Saving the same link again replaces the copy. |
| `recall` | Search memories and pages by natural-language query. |
| `distill` | Trigger page distillation for new clusters or a specific `page_id`. |
| `list_pending` | List pending captures waiting for review. |
| `confirm_memory` | Confirm a pending memory by `source_id`. |
| `forget` | Delete a memory by ID. Destructive. |
| `list_refinements` | Explicitly inspect the daemon's unique proposal queue, including `vocab_promote`; never polled ambiently. |
| `accept_refinement` | Accept one listed proposal after an unambiguous item-level decision. Local stdio only. |
| `reject_refinement` | Reject one listed proposal after an unambiguous item-level decision. Local stdio only. |

The refinement trio remains because this review queue has no CLI or replacement
path. Remote HTTP clients can list it, but `accept_refinement` and
`reject_refinement` are hidden and hard-rejected remotely.

`capture`, `write_page`, and `update_brief` accept an optional `session`
(`<tool>:<session id>`, for example `claude-code:<id>`). Wenlan records it so a
memory, note, or handoff links back to the conversation that wrote it. When the
argument is absent a local server uses `WENLAN_SESSION`, then `CODEX_THREAD_ID`
(as `codex:<id>`); a malformed value is ignored and never fails the write.

Lint repair is three tools behind `/wenlan lint repair`: `lint_repair_prepare` opens
and pages a plan or turns one Review Item into a manifest, `lint_repair_apply`
applies one approved manifest, and `lint_repair_verify` records the follow-up
check. All three are local stdio only. The agent-assisted Deep work packet is
paged through `lint` itself with `work_digest`.

Runtime diagnostics live in the CLI: `wenlan doctor`. They are not part of the
MCP memory loop.

### Query-only HTTP profile

For a connector that should retrieve knowledge without exposing knowledge-write
or maintenance tools, explicitly select the query-only profile:

```bash
WENLAN_SPACE=shared wenlan-mcp serve --tool-profile query-only --token-file /path/to/bearer-token
```

Native launchers can instead provide the token in a child-only environment
variable and pass `--token-env WENLAN_REMOTE_MCP_TOKEN`. This is mutually
exclusive with `--token`, `--token-file`, and `--no-auth`. The explicitly named
variable must contain 32-128 ASCII letters, digits, `_`, or `-`; missing or
invalid values stop startup without falling back to a token file. Do not put
the secret itself in command arguments or log the child's environment. This
keeps it out of command-line listings, not out of the child process memory or
the reach of an account that can inspect that process.

This profile advertises only `brief`, `recall`, and `get_page_sources`. The
server also rejects all other tool names before dispatch, including direct
calls to hidden tools. It requires a nonempty bearer token; `--no-auth` is not
permitted. A strict `WENLAN_SPACE` pin is also required; an overridable
`WENLAN_DEFAULT_SPACE` is not an authorization boundary. The default `standard`
profile keeps its existing response format.

Query-only also exposes `GET /connector-info` behind the same bearer and Origin
checks as `/mcp`. It reports contract version 1, `wenlan-mcp`, the query-only
profile, bearer authentication, and the pinned Space. Relay enrollment must
check that anonymous access fails and that the authenticated response matches
the intended Space. This is configuration verification, not proof of device
ownership. Standard mode has no such endpoint; public `/health` reveals no Space.

Query-only success results include `structuredContent` and matching JSON text;
each advertised tool has an output schema generated from its response type.
Search results retain source IDs, titles, content, and archive/review status.
Brief results retain summary, active/backlog text and gates, plus optional
related context. Page sources retain source IDs and available source content;
unavailable linked memories are omitted, including their IDs, because missing
content may be outside the granted Space. Raw import text, ranking and
access metadata are not forwarded by these projections. Stored content itself
can still contain personal data; projection is not content classification or
authorization. Page IDs must be opaque ASCII letters, digits, `_` or `-`, not
URLs or filesystem paths; this input check also applies to Standard callers.

"Query-only" describes the available knowledge operations, not an absence of
all side effects: `recall` still records the query and accessed memory IDs in
the daemon's private activity history. It therefore declares
`readOnlyHint: false` under the plugin submission rules.

This is a tool-access boundary, not OAuth or multi-user isolation. It does not
make a local daemon ready for public marketplace distribution. Public access
to private libraries still needs authenticated user-to-library routing,
authorization, consent, revocation, privacy disclosures, and end-to-end tests.
Do not publish a personal daemon or its bearer token as a universal service.

The repository's [`chatgpt-app-submission.json`](../../chatgpt-app-submission.json)
is a preparation draft for this profile, not the Standard tool inventory. It
contains listing suggestions, three annotation justifications, and five positive
and three negative test cases. The cases explicitly identify pending synthetic
fixtures and public-host execution. Schema validation is not a test pass, an
authentication implementation, or approval to upload or submit the draft.

### Web-write HTTP profile

`--tool-profile web-write` is the query-only profile plus the writes a user
allowed one web AI app to make. The desktop app runs it as a second process,
next to the query-only one, for apps that were approved with write access.
Startup requires the same bearer token and the same strict `WENLAN_SPACE` pin
or `--whole-library` as query-only, and `/connector-info` reports
`"tool_profile": "web-write"`.

It advertises exactly `brief`, `recall`, `get_page_sources`, `read_page`,
`list_pages`, `capture`, `write_page`, `rename_page`, `forget`, and
`delete_page`; any other name is refused before dispatch. `brief`, `recall`,
and `get_page_sources` answer with the query-only projection and output
schemas. In this profile only, the remote-connection blocks on `forget`,
`delete_page`, `rename_page`, and `write_page` refresh are lifted. Every other
remote restriction stays, including `code_basis`. `forget` and `delete_page`
declare `destructiveHint: true` so web clients ask before running them.

Under a one-Space pin, an id-based call is checked against that Space before it
acts. The daemon scopes `read_page` and `rename_page` by the Space header
itself; for `forget`, `delete_page`, `write_page` refresh, and `capture` with
`supersedes`, the server first reads the target through the same scoped route
and refuses when the daemon does not return it. A whole-library connection has
no pin and no such check.

## Setup Modes

Wenlan works immediately: storage, search, recall, pages your agent writes, and every MCP tool need no local model or API key.

Steep, the optional background upkeep (automatic extraction and page refresh), stays off until you turn it on with `wenlan steep configure`. It needs a model to run: `wenlan models install` provides an on-device one and `wenlan keys set anthropic` an API-backed one, but installing either does not turn Steep on.

## Agent Guidance

The server instructions tell agents to work wiki-first: read the Brief and the relevant pages before answering, write what they learn back, and hand off through `update_brief`. The capture policy (one idea per capture; include the why; name people, projects, and tools; omit `memory_type` unless certain; never store tool output, logs, or filler) lives in the `capture` tool description, so every client sees it at the point of use.

See [`src/tools.rs`](src/tools.rs) for the exact text.

## Links

- [wenlan.app](https://wenlan.app) — project home
- [wenlan.app/learn/mcp-memory-server](https://wenlan.app/learn/mcp-memory-server) — concept article on Wenlan as an MCP memory server
- [wenlan.app/docs/mcp-clients](https://wenlan.app/docs/mcp-clients) — connect Claude Code, Cursor, Codex, Claude Desktop, Gemini CLI
- [npm: wenlan-mcp](https://www.npmjs.com/package/wenlan-mcp) — standalone npm package
- [github.com/wenlan-app/wenlan](https://github.com/wenlan-app/wenlan) — source

## License

Apache-2.0.
