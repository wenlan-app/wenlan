# Contributing to Wenlan

Wenlan is an open-source knowledge base you don't have to maintain: your AI tools keep it organized. We welcome bug fixes, features, tests, docs, and design feedback.

This repo holds the daemon (`wenlan-server`), the CLI (`wenlan`), the MCP server (`wenlan-mcp`), the shared types/core (`wenlan-types`, `wenlan-core`), and the Tauri desktop app (`wenlan-app`, in `app/`). Bug reports for the local runtime, CLI, MCP server, desktop app, and plugin are welcome here.

## Quick start

```bash
git clone https://github.com/wenlan-app/wenlan.git && cd wenlan
pnpm install      # once
pnpm dev:all      # the desktop app, with its own daemon, port and data
```

Your installed Wenlan and its pages are never touched: `pnpm dev:all` keeps
everything it starts apart, and a debug app started without it refuses to run.

| Command | What it does |
|---|---|
| `pnpm dev:all` | Desktop app plus its own daemon |
| `pnpm dev:daemon` | Only the daemon, on its own port and data (`pnpm clean:dev` stops it) |
| `pnpm dev:team` | A team server, as Docker runs one, with scratch data; it prints the owner's invite link to join from `pnpm dev:all` |
| `pnpm test` | Frontend tests |
| `pnpm test:rust` | Daemon, CLI and MCP tests |
| `pnpm check` | Formatting, lint and type checks, as CI runs them |

The other scripts in `package.json` are for releases, review builds and
tooling; you do not need them to make a change.

## What you need

macOS arm64, Linux (x86_64 + arm64; glibc), or Windows x86_64, with:

- the platform's build tools ([Xcode Command Line Tools](https://developer.apple.com/xcode/resources/) on macOS, MSVC Build Tools on Windows, gcc + make on Linux);
- [Rust](https://rustup.rs/) (stable), [Node.js](https://nodejs.org/) and [pnpm](https://pnpm.io/);
- for the desktop app, [`cloudflared`](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) on `PATH` (or `CLOUDFLARED_BIN` pointing at it). Wenlan ships it for Remote Access, so `pnpm dev:all`, `pnpm build:all` and `pnpm tauri build` fail without it.

The first build takes several minutes while `llama.cpp` compiles.
macOS Intel is not a supported stock build target: the pinned ONNX Runtime
has no prebuilt x86_64 macOS binary. See the
[platform note](../crates/wenlan-cli/README.md#macos-intel).

### Without Node

The daemon, CLI and MCP server build with Cargo alone:

```bash
cargo build -p wenlan-server -p wenlan
cargo test --workspace --exclude wenlan-app
```

`cargo run -p wenlan-server` uses the same port and data folder as an
installed Wenlan. Stop the installed one first, or use `pnpm dev:daemon`.
To install your build as the per-user background service:

```bash
cargo build --release -p wenlan -p wenlan-server
./target/release/wenlan setup --basic
./target/release/wenlan background on
```

See [app/AGENTS.md](../app/AGENTS.md) for the desktop app's full workflow.

## Architecture Overview

- **Shared types**: `crates/wenlan-types` (Apache-2.0). Lightweight wire types shared with `wenlan-mcp` and `wenlan-app` as in-workspace path deps.
- **Core logic**: `crates/wenlan-core` (Apache-2.0). DB, embeddings, LLM engine, search, knowledge graph, distill cycles, eval. No tauri / no axum dependencies.
- **HTTP daemon**: `crates/wenlan-server` (Apache-2.0), serves `127.0.0.1:7878`.
- **CLI binary**: `crates/wenlan-cli` (Apache-2.0). The `wenlan` command for setup, service management, search, recall, etc.
- **MCP server**: `crates/wenlan-mcp` (Apache-2.0). The connector spawned by Claude Code, Cursor, Codex, and other MCP clients.
- **Desktop app**: `wenlan-app` in `app/` (AGPL-3.0-only). Tauri + React; folded into this monorepo on 2026-07-20.
- **Database**: libSQL (vectors + knowledge graph + FTS).

See `crates/wenlan-core/REFERENCE.md` and `crates/wenlan-server/REFERENCE.md` for the module-by-module breakdown.

## Finding Work

Look for issues labeled [`good first issue`](https://github.com/wenlan-app/wenlan/labels/good%20first%20issue) or [`help wanted`](https://github.com/wenlan-app/wenlan/labels/help%20wanted).

## Pull Request Process

1. Fork the repo and create a branch from `main`
2. Make your changes, keeping PRs small and focused (one logical change per PR)
3. Ensure all tests pass and linting is clean
4. Open a PR using the template, describing what changed and how to test it

CI runs `cargo fmt --check --all`, `cargo clippy --workspace --exclude wenlan-app --all-targets -- -D warnings`, and cargo-nextest slices across the daemon crates. The `wenlan-app` desktop crate is checked separately, after sidecar binaries are staged.

## Code Conventions

These conventions keep the codebase consistent.

- **SQL safety**: Always use parameterized queries. Never interpolate user input into SQL strings.
- **NULL semantics**: Store `Option<T>` as SQL NULL, not empty string
- **UTF-8 safety**: Never byte-index Rust strings (`&s[..n]`). Use `chars().take(n)` instead.
- **Batch SQL**: Wrap multi-row insert/delete loops in `BEGIN`/`COMMIT` transactions
- **License headers**: The workspace is still normalizing SPDX headers after the package split. For new files, use the header that matches the package/file license even if nearby legacy files have not been cleaned up yet.

## Docs Layout

- In-repo docs live under `docs/` and are documentation only. Nothing there is read by code or CI.
- Working design docs (plans, specs, research) are deliberately not tracked. `docs/plans/` and `docs/superpowers/` are gitignored local scratch space, so nothing you need to contribute lives there.
- `crates/wenlan-core/contracts/` holds a markdown file that is a test fixture rather than documentation: `crates/wenlan-core/src/truth_manifest_test.rs` compiles it in with `include_str!`. Editing it changes what a test asserts.

## License

This repo is Apache-2.0: `crates/wenlan-types`, `crates/wenlan-core`, `crates/wenlan-server`, `crates/wenlan-cli`, `crates/wenlan-mcp`, and the Claude Code plugin files. The `wenlan-app` desktop crate (`app/`) is AGPL-3.0-only via its own `license` field.

By contributing, you agree that your changes will be licensed under the license that applies to the files you modify.

## Links

- [wenlan.app](https://wenlan.app): project home
- [wenlan.app/docs/get-started](https://wenlan.app/docs/get-started): install and verify the local memory loop before opening a PR
- [wenlan.app/docs/daily-workflow](https://wenlan.app/docs/daily-workflow): the workflow your changes will fit into
