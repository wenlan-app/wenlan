---
title: "How to run it"
type: page
description: "From a fresh clone to a running daemon, the desktop app via pnpm dev:all, and a throwaway daemon with its own port, data dir and pages folder."
tags: ["wenlan"]
space: "wenlan"
origin_id: page_c4655a82-3947-46b3-b0a5-4610ca1907d2
origin_version: 1
created: 2026-10-23
modified: 2026-10-23
generated: {by: "wenlan/0.20.0+g279fad04", at: "2026-10-23T18:22:35.784806+00:00"}
status: stable
sources:
  - {resource: "repo://.claude/skills/run-wenlan/SKILL.md#L43-L53"}
  - {resource: "repo://.github/workflows/ci.yml#L2686-L2692"}
  - {resource: "repo://AGENTS.md#L50-L80"}
  - {resource: "repo://Cargo.toml#L11-L21"}
  - {resource: "repo://crates/wenlan-core/src/config.rs#L239-L247"}
  - {resource: "repo://crates/wenlan-server/src/main.rs#L667-L672"}
  - {resource: "repo://docs/setup-and-data.md#L22-L46"}
  - {resource: "repo://package.json#L7-L35"}
  - {resource: "repo://rust-toolchain.toml#L1-L4"}
  - {resource: "repo://scripts/dev-all.sh#L45-L56"}
  - {resource: "repo://scripts/dev-runtime.sh#L222-L239"}
  - {resource: "repo://scripts/dev-runtime.sh#L2232-L2257"}
  - {resource: "repo://scripts/install-git-hooks.mjs#L9-L37"}
  - {resource: "repo://scripts/prepare-sidecars.sh#L74-L92"}
  - {resource: "repo://scripts/run-bash.mjs#L4-L14"}
  - {resource: "repo://scripts/run-tauri.mjs#L4-L7"}
---

Wenlan builds on macOS arm64, Linux (x86_64 and aarch64) and Windows x86_64. The Rust crates need only Rust; the desktop app's frontend adds Node and pnpm. This page goes from a fresh clone to a running daemon, then the desktop app, then a throwaway daemon you can break safely.

## Prerequisites

- Rust 1.95.0, pinned in `rust-toolchain.toml` together with rustfmt, clippy and rust-analyzer.
- pnpm 10.28.2, pinned by `packageManager` in `package.json`. CI runs the app checks on Node 24.
- On Windows the pnpm scripts go through `scripts/run-bash.mjs`, which looks for Git Bash. The `bash` on PATH may be WSL, which cannot see the Windows toolchain.

## Install and build

Run `pnpm install` once. Its `postinstall` runs `scripts/install-git-hooks.mjs`, which points `core.hooksPath` at `.githooks` and marks the hooks executable. It is best effort, so it never fails the install.

A bare `cargo build` covers only the five daemon crates; the desktop app is selected with `-p wenlan-app`. `cargo build -p wenlan-server -p wenlan-mcp -p wenlan` builds the daemon, the MCP bridge and the CLI.

## Start a daemon

With nothing set, the daemon listens on port 7878 and keeps its data under `wenlan` in the platform's local data directory. That is the installed app's port and data, so develop against the isolated runtime instead:

```bash
pnpm dev:daemon   # build wenlan-server and start a worktree-owned daemon
pnpm clean:dev    # stop it
```

`scripts/dev-runtime.sh` derives its settings from a checksum of the checkout path. The daemon port is 17000 plus that number modulo 1000, the UI port is 18000 plus the same, and the data dir sits under `$TMPDIR/wenlan-app-dev/<id>/data`. It builds `wenlan-server` with cargo and starts it with `WENLAN_PORT` and `WENLAN_DATA_DIR` set.

## Desktop app

`pnpm dev:all` is the supported way to run the app. It runs `pnpm prepare:sidecars --force-build`, starts the worktree daemon (or reuses a healthy one), then launches `pnpm tauri dev` with that worktree's app id and UI port. `pnpm tauri` is itself a wrapper, `scripts/run-tauri.mjs`, which points the Tauri CLI at `app/`.

The sidecars are the three binaries the app bundles. `pnpm prepare:sidecars` (also `pnpm build:daemon`) builds `wenlan-server`, `wenlan-mcp` and `wenlan` unless all three already exist, then copies them into `app/binaries`.

## A throwaway daemon

To drive the CLI or MCP bridge against a daemon you can discard, give it its own port, data dir and pages folder. Write the config before the first start, as the run-wenlan skill does:

```bash
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/run.XXXXXX")"
mkdir -p "$DATA_DIR/pages"
printf '{"knowledge_path":"%s"}' "$DATA_DIR/pages" >"$DATA_DIR/config.json.tmp"
mv "$DATA_DIR/config.json.tmp" "$DATA_DIR/config.json"
WENLAN_NO_AUTOSTART=1 WENLAN_PORT=17878 WENLAN_DATA_DIR="$DATA_DIR" ./target/debug/wenlan-server &
curl -sf http://127.0.0.1:17878/api/health          # ready
curl -sf http://127.0.0.1:17878/api/knowledge/path  # must be $DATA_DIR/pages
```

Then point tools at it. The CLI reads `WENLAN_HOST` as a full URL, for example `WENLAN_HOST=http://127.0.0.1:17878`. `wenlan-mcp` takes `--origin-url`. Put `WENLAN_NO_AUTOSTART=1` on every harness command so a failed connect cannot start your real background service.

## Installing as a user

End users skip all of this. On macOS Apple Silicon they run `npx -y wenlan setup`. Without Node they fetch and run `install.sh`, then run `wenlan setup --basic`. These download prebuilt binaries, so no Rust toolchain is needed. The first launch also downloads a model for local search.

Next: [The HTTP daemon](the-http-daemon.md), The CLI, [Desktop app](desktop-app.md), Configuration and environment and [How tests run](how-tests-run.md). [Known pitfalls](known-pitfalls.md) lists what trips newcomers up.

## Code

- [`.claude/skills/run-wenlan/SKILL.md` lines 43-53](../../.claude/skills/run-wenlan/SKILL.md#L43-L53)
- [`.github/workflows/ci.yml` lines 2686-2692](../../.github/workflows/ci.yml#L2686-L2692)
- [`AGENTS.md` lines 50-80](../../AGENTS.md#L50-L80)
- [`Cargo.toml` lines 11-21](../../Cargo.toml#L11-L21)
- [`crates/wenlan-core/src/config.rs` lines 239-247](../../crates/wenlan-core/src/config.rs#L239-L247)
- [`crates/wenlan-server/src/main.rs` lines 667-672](../../crates/wenlan-server/src/main.rs#L667-L672)
- [`docs/setup-and-data.md` lines 22-46](../../docs/setup-and-data.md#L22-L46)
- [`package.json` lines 7-35](../../package.json#L7-L35)
- [`rust-toolchain.toml` lines 1-4](../../rust-toolchain.toml#L1-L4)
- [`scripts/dev-all.sh` lines 45-56](../../scripts/dev-all.sh#L45-L56)
- [`scripts/dev-runtime.sh` lines 222-239](../../scripts/dev-runtime.sh#L222-L239)
- [`scripts/dev-runtime.sh` lines 2232-2257](../../scripts/dev-runtime.sh#L2232-L2257)
- [`scripts/install-git-hooks.mjs` lines 9-37](../../scripts/install-git-hooks.mjs#L9-L37)
- [`scripts/prepare-sidecars.sh` lines 74-92](../../scripts/prepare-sidecars.sh#L74-L92)
- [`scripts/run-bash.mjs` lines 4-14](../../scripts/run-bash.mjs#L4-L14)
- [`scripts/run-tauri.mjs` lines 4-7](../../scripts/run-tauri.mjs#L4-L7)

