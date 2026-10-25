# Wenlan Skills

Claude Code workflow skills installed by the Wenlan plugin.

The skills do not store data themselves. They guide Claude Code to use the local `wenlan-mcp` tools, which talk to the Wenlan daemon on `127.0.0.1:7878`.

These files are also the source for the Codex plugin: `scripts/gen-codex-plugin.mjs` turns each menu skill here into `plugin-codex/skills/<skill>/SKILL.md` and each workflow reached through `/wenlan` into `plugin-codex/skills/wenlan/references/<skill>.md`, applying `plugin-codex/overlay.json` for the parts Codex does differently. Edit here, then regenerate.

They are also the source for the host-neutral Agent Skills the Wenlan app installs into other AI tools (Cursor, GitHub Copilot, VS Code, Windsurf, Antigravity, Kiro, OpenCode, Kimi Code, Gemini CLI): `scripts/gen-agent-skills.mjs` writes `plugin-agents/skills/` (the `wenlan` front door with its workflow references, plus `wenlan-brief`, `wenlan-capture`, `wenlan-recall` and `wenlan-handoff`) and the `app/src/agent_skills_bundle.rs` that embeds them, applying `plugin-agents/overlay.json`. Run both generators after editing a skill here; CI runs each with `--check`.

Skills must write `${CLAUDE_PLUGIN_ROOT}` with braces and no `:-` default: Claude Code substitutes that exact form when it loads the skill, and does not export the variable to the Bash tool, so `$CLAUDE_PLUGIN_ROOT` is empty at run time.

## Commands

<!-- verbs:start -->
<!-- generated from plugin/skills/verbs.json by scripts/gen-plugin-verbs.mjs; do not edit -->

| Command | Purpose |
| --- | --- |
| `/wenlan [what you want]` | the front door: say what you want, or type it bare for status and setup |
| `/brief [topic]` | load the Brief for the current Space (optional; not a boot step) |
| `/capture <x>` | save one durable memory in flow, or this conversation as one note on request |
| `/recall <q>` | search local memory |
| `/handoff` | end-of-session debrief: captures, session log, Brief update |
| `/wenlan setup` | set up or repair Wenlan (installs the local runtime) |
| `/wenlan lint [deep\|repair] [scope]` | diagnose, or resolve all findings safely |
| `/wenlan distill [t]` | advanced: synthesize or refresh source-backed pages for the active Space |
| `/wenlan steep [space:<name>]` | write pages from new material and bring out of date ones up to date with this AI tool (steep on demand) |
| `/wenlan wiki [space:<name>]` | read this repository and write a wiki about it, sized to the repository, each page tied to the code lines it relies on |
| `/wenlan pages [q]` | browse and open distilled pages |
| `/wenlan curate <surface>` | deep audit of captures, revisions or refinements |
| `/wenlan forget <id>` | delete one memory by ID, after confirmation |
<!-- verbs:end -->

Plugin metadata lives in [`.claude-plugin/plugin.json`](../.claude-plugin/plugin.json); the listing README is [`../README.md`](../README.md).

## Choosing the active space

Every space-aware skill resolves the active memory bucket through the
ordered chain below. Higher layers override lower ones:

| Layer | Mechanism | Example |
|---|---|---|
| 1 | `WENLAN_SPACE` strict process pin | `WENLAN_SPACE=career claude` |
| 2 | `space:X` inline arg | `/capture space:health "slept 5hrs"` |
| 3 | `WENLAN_DEFAULT_SPACE` overridable process context | `WENLAN_DEFAULT_SPACE=career claude` |
| 4 | `~/.wenlan/spaces.toml` cwd-prefix mapping (longest prefix wins; ties go to first-defined) | see `plugin/examples/spaces.toml` |
| 5 | registered cwd git-repo basename | `~/Repos/wenlan/...` → `wenlan`, only if that Space exists |
| 6 | none | omit client context; daemon Default decides new writes |

To pin a session to a specific bucket regardless of cwd, set
`WENLAN_SPACE` before invoking Claude Code. To pin by working directory
declaratively, copy `plugin/examples/spaces.toml` to
`~/.wenlan/spaces.toml` and edit. To override per call, prefix any
space-aware skill arg with `space:<name>`.

On each space-aware skill call, the resolver prints one line naming the
chosen Space and the layer it came from, so the user can confirm the active
bucket.

If the resolver reports no space, the skill omits the space parameter. The
daemon then applies its Default save space to new writes; reads remain All
Spaces. The tool's returned receipt is authoritative if it differs from the
client's proposed context.

## Links

- [wenlan.app](https://wenlan.app) — project home
- [crates/wenlan-mcp/README.md](../../crates/wenlan-mcp/README.md) — MCP tools reference
- [crates/wenlan-cli/README.md](../../crates/wenlan-cli/README.md) — CLI subcommand reference, generated from the clap definitions
- [wenlan.app/docs/daily-workflow](https://wenlan.app/docs/daily-workflow) — brief/capture/recall/handoff loop
- [wenlan.app/learn/claude-code-memory](https://wenlan.app/learn/claude-code-memory) — Claude Code memory concept article
