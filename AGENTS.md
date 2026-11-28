# AGENTS.md

Wenlan is a local-first agent memory system. This monorepo ships the daemon, CLI,
shared wire types, business-logic core, MCP server, and Tauri desktop app.

Keep this file to repository-wide behavior. Detailed facts, flag receipts, eval
methodology, and historical decisions belong in the owning reference or runbook and
are read only when the task needs them.

## Start here

| Work | Read |
|---|---|
| Business logic, storage, retrieval, enrichment | `crates/wenlan-core/AGENTS.md` |
| HTTP daemon and runtime workers | `crates/wenlan-server/AGENTS.md` |
| Desktop app and frontend | `app/AGENTS.md` or the closest nested `AGENTS.md` |
| Anything that changes what a person sees or how an AI tool behaves for them | `docs/ux/spec.md` (agreed UX rules, each with an id and a test) |
| React frontend (Tauri UI) | `src/AGENTS.md` |
| Browser e2e tests | `e2e/AGENTS.md` |
| Browser-only preview harness | `preview/AGENTS.md` |
| Release, sidecar, and repo-inventory scripts | `scripts/AGENTS.md` |
| Eval fixtures and artifacts | `app/eval/AGENTS.md` |
| Rust eval runners and experiment design | `crates/wenlan-core/src/eval/AGENTS.md` |
| Test routing and CI layers | `docs/test-layers.md` |
| Platform and release work | `docs/cross-platform.md`, `docs/RELEASING.md` |
| Live daemon/app verification | `.claude/skills/run-wenlan/SKILL.md`, then the relevant verify/prove skill |

## Build and verification

The default Cargo workspace covers the five daemon crates; the desktop app must be
selected explicitly with `-p wenlan-app`.

- During implementation, run the smallest test that exercises the changed behavior.
- Before publishing a stable head, run the affected-package and reverse-dependency
  closure selected by the existing CI planner, or locally via `WENLAN_PUSH_FULL=1
  git push` (pre-push is fast/non-compiling by default; CI is the required gate).
- The full workspace suite is the CI/main backstop, not a per-worker or per-review-pass
  default. Do not rerun an unchanged, trustworthy receipt without a concrete reason.
- Eval quality runs, live GPU smokes, and deployed checks are measurement lanes. Run
  them only when the change or requested evidence requires them.
- Green tests do not verify a change to what a person sees or does (a screen, wording,
  a flow). Open the product, walk the changed screens as the user would, screenshot
  them, have an independent agent review the screenshots, fix what it finds, then
  report. Use the browser preview (`preview/AGENTS.md`) unless the thing exists only
  in the native app (window, tray, install/update, system file picker, native
  permissions; `.claude/skills/run-wenlan-app/SKILL.md`). Use test data, never the
  user's real data or accounts, and never publish, share, invite, send, delete, or
  press a person-only confirm during the check. A change nobody sees (backend, CI,
  docs, internal logic) opens no screen; decide by need, not habit.
- A missing fixture or unavailable gate is `unchecked`, never a pass. Follow
  `docs/ci-flake-policy.md` before rerunning or quarantining an intermittent CI failure.

## Evidence in reports

- Read the source before claiming how it works; cite only files and lines you inspected.
- Keep configured, available, exercised, and successful apart, and separate observed
  facts from proposals and unchecked claims.
- Before saying something cannot be done, is broken, or must wait, check it yourself.
- Before giving anyone a command or lines to paste, run them yourself on test data.
- Pass on what another session or worker reports only after checking it.

## Writing changes

- A rare case the change cannot handle safely (a crash mid-write, an unusual permission,
  a concurrent edit) is refused with a clear reason and left untouched, with one test for
  the refusal. Add handling for it only when real use shows people hit it.
- Do not add compatibility code for state that no release ever produced; refuse it and
  say how to start over.
- Put setup that tests repeat (a fake home, tool folders, a scratch daemon) in a shared
  helper. Reuse or extend the helper instead of copying setup into a new test.

Hooks install automatically via `package.json`'s `postinstall`
(`scripts/install-git-hooks.mjs`) when you run `pnpm install`. The manual fallback for
a checkout without `pnpm install` is `git config core.hooksPath .githooks`. Do not
claim hooks are active without checking the current worktree.

## Repository invariants

- `wenlan-core` stays framework-agnostic: no Tauri or Axum dependencies.
- `wenlan-types` stays lightweight: its production dependencies are only `anyhow`,
  `serde`, and `serde_json`. Shared wire shapes live there; do not add `chrono`,
  `tokio`, or other heavy dependencies.
- `wenlan-server` owns HTTP framing and the single database writer. Business logic
  belongs in `wenlan-core`; the app and CLI use daemon APIs.
- MCP wrappers typed-deserialize daemon responses; do not pass arbitrary
  `serde_json::Value` envelopes through the boundary.
- Never hold a `tokio::sync::RwLock` guard across `.await`; snapshot state and drop the
  guard first.
- Canonical write-time enrichment routing: see `crates/wenlan-core/AGENTS.md` ("Core
  invariants").
- Dev and production use the same daemon port and data roots by default. Isolated tests
  that can create pages must isolate the daemon port, `WENLAN_DATA_DIR`, and the
  configured knowledge/page path, then verify the live daemon reports those scratch
  paths. For a scratch daemon, the CLI reads `WENLAN_HOST` (a full URL such as
  `http://127.0.0.1:17917`, not a port), `wenlan-mcp` takes `--origin-url`, and a
  `WENLAN_DATA_DIR` other than the installed app's root puts the default pages folder
  at `$WENLAN_DATA_DIR/pages`; such a daemon refuses a `knowledge_path` inside the
  real pages folder.
- Request defaults: `WENLAN_AGENT_NAME` supplies the CLI identity when no explicit
  agent name is given; `WENLAN_DEFAULT_SPACE` is an overridable CLI/MCP fallback, while
  the strict `WENLAN_SPACE` lock wins over both explicit and default values. With none
  of them, the CLI and the stdio MCP server use the Space their working folder points
  at (`wenlan_types::folder_space`); a test that must not pick one runs outside a
  repository whose folder name is a registered Space.
- Daemon recovery: on a connect failure to a loopback host the CLI starts the registered
  background service once and re-polls health before failing. `WENLAN_NO_AUTOSTART=1`
  turns that off — every test harness and CI job must set it so a run can never start the
  developer's daemon. The `autostart.off` marker file in the data root is the persistent
  equivalent (`wenlan background off` writes it).
- Preserve cross-platform behavior on macOS arm64, Linux x86_64/aarch64, and Windows
  x86_64. Read `docs/cross-platform.md` before changing platform conditionals or release
  matrices.
- For platform bugs, check the equivalent Windows, macOS, and Linux paths in
  the same change. Add regression coverage for each applicable platform; never
  treat one platform's pass as evidence for another, and report untested paths.

## Git and release

Coding agents do not open GitHub issues on their own. Put flaky tests, follow-ups,
and ideas in the PR description or the task's handoff instead. Open an issue only
when the maintainer asks for one in that task, or a workflow under
`.github/workflows/` opens it by design.

Use conventional PR titles because release-please uses the squash title for the
changelog. Version changes must keep `version.txt`, `.release-please-manifest.json`, the
workspace version, and the app version surfaces in lockstep; the existing verifier is
the authority.

PRs land on `main` only through the merge queue (`gh pr merge <number>` enqueues), so
never update a branch and rerun CI by hand to land it. Queue the Release PR alone and
last, as `docs/RELEASING.md` describes.

Removing a worktree also removes its gitignored artifacts. Before cleanup, check for
the only copy of eval databases, model downloads, or other valuable caches and migrate
them to their documented external cache location.

## Documentation ownership

An `AGENTS.md` instruction must change agent behavior in that directory. Module maps,
environment-flag wiring, experimental receipts, benchmark statistics, and long-form
rationale go in the closest `REFERENCE.md` or existing `docs/` page. Link to them from
the short instruction file; do not copy them back into always-loaded context.
