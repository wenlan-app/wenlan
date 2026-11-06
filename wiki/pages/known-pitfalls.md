---
title: "Known pitfalls"
type: page
description: "Traps a newcomer hits: dev and production share port and data, the CLI autostarts the real service, tools default to :7878, old daemons win the port, and a few build and code rules."
tags: ["wenlan"]
space: "wenlan"
origin_id: page_d214ef14-4f99-435f-9d8a-06fc93f7dce0
origin_version: 1
created: 2026-10-23
modified: 2026-10-23
generated: {by: "wenlan/0.20.0+g279fad04", at: "2026-10-23T18:22:51.759742+00:00"}
status: stable
sources:
  - {resource: "repo://.claude/skills/run-wenlan/SKILL.md#L179-L194"}
  - {resource: "repo://.githooks/pre-push#L4-L8"}
  - {resource: "repo://.github/workflows/ci.yml#L2693-L2702"}
  - {resource: "repo://AGENTS.md#L50-L77"}
  - {resource: "repo://AGENTS.md#L93-L96"}
  - {resource: "repo://Cargo.toml#L11-L21"}
  - {resource: "repo://Cargo.toml#L57-L88"}
  - {resource: "repo://app/src/lib.rs#L715-L721"}
  - {resource: "repo://crates/wenlan-cli/src/client.rs#L4-L8"}
  - {resource: "repo://crates/wenlan-cli/src/client/recovery.rs#L50-L71"}
  - {resource: "repo://crates/wenlan-core/src/lint/serving_review_test.rs#L165-L169"}
  - {resource: "repo://crates/wenlan-mcp/src/client.rs#L18-L28"}
  - {resource: "repo://crates/wenlan-mcp/src/main.rs#L21-L23"}
  - {resource: "repo://crates/wenlan-server/AGENTS.md#L21-L33"}
  - {resource: "repo://crates/wenlan-server/src/ingest_routes.rs#L246-L253"}
  - {resource: "repo://crates/wenlan-server/src/main.rs#L702-L721"}
  - {resource: "repo://crates/wenlan-types/Cargo.toml#L15-L18"}
---

These are the traps a newcomer is most likely to hit, each confirmed in code or a repo doc. Most come from one fact: a dev build and the installed app share the same defaults, so a careless run touches your real memory. [How to run it](how-to-run-it.md) has the safe setup.

- **Dev and production share the same port and data.** Isolated tests that can create pages must isolate the daemon port, `WENLAN_DATA_DIR` and the pages folder, then check that the live daemon reports the scratch paths. A `WENLAN_DATA_DIR` other than the installed app's root puts the default pages folder at `$WENLAN_DATA_DIR/pages`, and such a daemon refuses a `knowledge_path` inside the real pages folder. Use `pnpm dev:daemon` or a scratch daemon. See [Data and storage](data-and-storage.md).
- **The CLI starts your real background service.** After a connect failure to a loopback host, the CLI starts the registered service once and polls its health. Set `WENLAN_NO_AUTOSTART=1` in every test harness and CI job. The `autostart.off` marker in the data root, written by `wenlan background off`, is the persistent switch.
- **Tools can quietly talk to the wrong daemon.** The CLI reads `WENLAN_HOST`, a full URL such as `http://127.0.0.1:17917`, and does not read `WENLAN_PORT`. `wenlan-mcp` reads no environment variable for this: unless you pass `--origin-url` it uses `http://127.0.0.1:7878`. See The CLI and [MCP server](mcp-server.md).
- **A new build can lose to an old daemon.** If a healthy daemon already holds the port, a second `wenlan-server` exits (cleanly, or with code 75 under launchd) instead of replacing it. Cargo can also print "Finished" after a fast-forward without recompiling. Touch `crates/wenlan-server/src/router.rs`, rebuild, then run `wenlan restart`. See [The HTTP daemon](the-http-daemon.md).
- **The debug desktop app refuses to start on its own.** In a debug build, `run()` checks runtime isolation first and panics with "unsafe debug runtime refused" when the check fails, telling you to start the app with `pnpm dev:all`. See [Desktop app](desktop-app.md).
- **Plain cargo skips the desktop app, and the app needs sidecars.** Default members are the five daemon crates, so a bare `cargo test` never compiles `app/`; use `-p wenlan-app`. Tauri's build script resolves the three sidecars at compile time, so CI creates empty placeholders; run `pnpm prepare:sidecars` for real ones. See [How tests run](how-tests-run.md).
- **A passing push is not a passing PR.** By default pre-push runs fast checks only, and CI owns clippy, tests and the route catalog. `WENLAN_PUSH_FULL=1 git push` adds the local affected-closure run.
- **A new daemon route that reads user data needs two registrations.** Add it to `crates/wenlan-core/src/lint/serving/routes.rs` and to `SCOPED` or `GLOBAL` in `serving_review_test.rs`; the route-catalog test asserts both sets exactly. CI always runs that wenlan-core lib test, and pre-push runs it only under `WENLAN_PUSH_FULL=1`. See Lint and repair.
- **Never hold a `tokio::sync::RwLock` guard across `.await`.** Handlers snapshot the `Arc<MemoryDB>` in a short block and drop the guard first, because tokio's write-preferring lock would queue every other handler behind a slow upsert.
- **Keep `wenlan-types` light.** Its only production dependencies are `anyhow`, `serde` and `serde_json`. Do not add `chrono`, `tokio` or other heavy crates; shared wire shapes go there and business logic goes in `wenlan-core`.
- **Stay on rustls.** The workspace turns off reqwest's default features and selects `rustls-tls`, because the defaults pull in native-tls and openssl-sys, which fails in the cross-rs Linux container. A vendored `openssl-sys` is declared as a fallback. When you add an HTTP dependency, disable default features and pick its rustls feature. See Release process.
- **Removing a worktree deletes its gitignored files.** That includes eval databases and model downloads. Check for the only copy first and migrate it to its documented external cache location.

## Code

- [`.claude/skills/run-wenlan/SKILL.md` lines 179-194](../../.claude/skills/run-wenlan/SKILL.md#L179-L194)
- [`.githooks/pre-push` lines 4-8](../../.githooks/pre-push#L4-L8)
- [`.github/workflows/ci.yml` lines 2693-2702](../../.github/workflows/ci.yml#L2693-L2702)
- [`AGENTS.md` lines 50-77](../../AGENTS.md#L50-L77)
- [`AGENTS.md` lines 93-96](../../AGENTS.md#L93-L96)
- [`Cargo.toml` lines 11-21](../../Cargo.toml#L11-L21)
- [`Cargo.toml` lines 57-88](../../Cargo.toml#L57-L88)
- [`app/src/lib.rs` lines 715-721](../../app/src/lib.rs#L715-L721)
- [`crates/wenlan-cli/src/client.rs` lines 4-8](../../crates/wenlan-cli/src/client.rs#L4-L8)
- [`crates/wenlan-cli/src/client/recovery.rs` lines 50-71](../../crates/wenlan-cli/src/client/recovery.rs#L50-L71)
- [`crates/wenlan-core/src/lint/serving_review_test.rs` lines 165-169](../../crates/wenlan-core/src/lint/serving_review_test.rs#L165-L169)
- [`crates/wenlan-mcp/src/client.rs` lines 18-28](../../crates/wenlan-mcp/src/client.rs#L18-L28)
- [`crates/wenlan-mcp/src/main.rs` lines 21-23](../../crates/wenlan-mcp/src/main.rs#L21-L23)
- [`crates/wenlan-server/AGENTS.md` lines 21-33](../../crates/wenlan-server/AGENTS.md#L21-L33)
- [`crates/wenlan-server/src/ingest_routes.rs` lines 246-253](../../crates/wenlan-server/src/ingest_routes.rs#L246-L253)
- [`crates/wenlan-server/src/main.rs` lines 702-721](../../crates/wenlan-server/src/main.rs#L702-L721)
- [`crates/wenlan-types/Cargo.toml` lines 15-18](../../crates/wenlan-types/Cargo.toml#L15-L18)

