---
okf_version: "0.2"
---

## wenlan

* [Data and storage](pages/data-and-storage.md) - Memories, pages and links live in a local libSQL database under the data root; pages are also projected to Markdown files, reconciled at startup and watched for edits.
* [Desktop app](pages/desktop-app.md) - The desktop app is a Tauri 2 crate in app/ plus a React 19 UI in src/; it keeps no database and reaches the daemon only through WenlanClient.
* [How a page is written](pages/how-a-page-is-written.md) - One write_page call becomes a POST or PUT to the daemon's page route, which checks version, code lines and citations, stages a revision card for locked or owned pages, and otherwise commits one transaction plus the Markdown file.
* [How search works](pages/how-search-works.md) - Memory search fuses a BGE vector search and an FTS5 keyword search with weighted RRF, scales by quality signals, adds a graph boost and an optional cross-encoder; pages are looked up separately and capped at 3.
* [How steep keeps pages current](pages/how-steep-keeps-pages-current.md) - Steep has two modes (Automatic and On demand, never off) that share one claim, snapshot and fenced-commit flow, and it rewrites pages whose sources or cited code changed.
* [How tests run](pages/how-tests-run.md) - Fast local hooks (fmt, Clippy, a reader-inventory check) gate commits and pushes; CI runs a fail-closed differential plan with one required conclusion job; quality runs and live smokes never gate.
* [How to run it](pages/how-to-run-it.md) - From a fresh clone to a running daemon, the desktop app via pnpm dev:all, and a throwaway daemon with its own port, data dir and pages folder.
* [Known pitfalls](pages/known-pitfalls.md) - Traps a newcomer hits: dev and production share port and data, the CLI autostarts the real service, tools default to :7878, old daemons win the port, and a few build and code rules.
* [MCP server](pages/mcp-server.md) - wenlan-mcp serves 35 Wenlan tools over stdio or Streamable HTTP, reaches the daemon over HTTP with typed responses, and honors WENLAN_SPACE as a strict Space lock.
* [Module map](pages/module-map.md) - Five daemon crates plus the Tauri app form the Cargo workspace; the CLI, MCP server and app reach the daemon over HTTP, and the daemon runs the core library over wenlan-types.
* [The HTTP daemon](pages/the-http-daemon.md) - wenlan-server is one local Axum process: it binds 127.0.0.1:7878 before touching data, serves a tracked router over shared state, and runs the outbox drain and a 30-second scheduler.
* [What Wenlan is](pages/what-wenlan-is.md) - Wenlan is a local-first personal wiki and agent memory system; this monorepo ships its daemon, CLI, shared types, core library, MCP server, desktop app, plugins and web relay.
