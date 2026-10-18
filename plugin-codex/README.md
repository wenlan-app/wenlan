# Wenlan Codex plugin

The Codex plugin wraps the shared `wenlan-mcp` server with Codex slash skills.
Use it when you want the session workflow commands in Codex, not just raw MCP
tools.

## Install

Install the runtime using the host-specific path in [Set up Wenlan with your AI client](../docs/setup-with-ai.md). On macOS Apple Silicon, that step is:

```bash
npx -y wenlan setup
```

Then install the Codex plugin:

```bash
codex plugin marketplace add wenlan-app/wenlan
codex plugin add wenlan@7xuanlu-wenlan
```

Start a new Codex thread after installing so the skills and MCP server load.
Then type `$wenlan` (or the exact form `$wenlan:wenlan`) to check that Wenlan
works, or `$wenlan` followed by what you want in plain words, such as
`$wenlan open my note on the release plan` or `$wenlan lint`. Saving, looking
things up, catching up and ending a session also have their own skills:
`/capture <memory>`, `/recall <query>`, `/brief`, and `/handoff`.

## Development

After editing `plugin-codex`, reinstall it into Codex's plugin cache:

```bash
python3 ~/.codex/skills/.system/plugin-creator/scripts/update_plugin_cachebuster.py plugin-codex
codex plugin add wenlan@7xuanlu-wenlan
```

The plugin runner uses `~/.wenlan/bin/wenlan-mcp` when available and falls back
to the pinned `npx` version in
[`bin/wenlan-mcp-runner.sh`](bin/wenlan-mcp-runner.sh). It passes
`--agent-name codex` so captures are labeled as Codex writes.

The skills under `skills/` are generated. Edit the Claude skill of the same
name in `plugin/skills/<skill>/SKILL.md`. Workflows hidden from the Claude menu
(setup, distill, pages, lint, curate, forget) are generated as reference files
under `skills/wenlan/references/`, because Codex cannot hide a skill; put anything Codex does differently
(a dropped section, a replaced paragraph, the `agents/openai.yaml` text) in
[`overlay.json`](overlay.json); then run:

```bash
node scripts/gen-codex-plugin.mjs
```

CI runs the same script with `--check`, so a hand edit to a generated file fails
the plugin job.

Before changing plugin skills, manifests, MCP runner wiring, or the local
marketplace, run the shared Claude/Codex contract checks:

```bash
python3 scripts/validate-codex-plugin-slice.py
python3 scripts/validate-plugin-contract.py
bash scripts/validate-plugin-contract.test.sh
```
