#!/usr/bin/env python3
"""Validate the hand-authored Codex plugin surface.

This is intentionally narrower than the Codex plugin validator. It checks the
Wenlan-specific surface contract before any shared generator exists.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / "plugin-codex"
REQUIRED_SKILL_INTERFACE = {
    "wenlan": {
        "display_name": "Wenlan",
        "short_description": "Say what you want from Wenlan, or check that it is working",
    },
    "brief": {
        "display_name": "Wenlan Brief",
        "short_description": "Read the Space Brief and optional related context",
    },
    "capture": {
        "display_name": "Wenlan Capture",
        "short_description": "Save a durable memory to Wenlan from the current conversation",
    },
    "handoff": {
        "display_name": "Wenlan Handoff",
        "short_description": "End a Codex session with captures and a typed Brief update",
    },
    "recall": {
        "display_name": "Wenlan Recall",
        "short_description": "Search Wenlan memories from Codex",
    },
}
# Workflows hidden from the Claude menu are reference files under the wenlan
# skill in Codex (plugin-contract.json `hidden_routed`), not separate skills.
REQUIRED_REFERENCES = {"setup", "pages", "lint", "curate", "forget", "distill", "wiki", "steep"}
SKILLS_WITHOUT_MCP_REFERENCE = {"handoff", "pages"}
SKILLS_USING_RESOLVER = {"brief", "capture", "distill", "handoff", "lint", "wiki", "recall", "steep"}
# Installed plugins live under ~/.codex/plugins/cache/<marketplace>/wenlan/<version>/;
# the checkout-relative `plugin-codex/bin/...` path only resolves inside this repo.
RESOLVER_LOCATOR = (
    "find \"$HOME/.codex/plugins/cache\" -path '*/wenlan/*/bin/resolve-space.sh'"
)
REQUIRED_GUARDRAILS = {
    "wenlan": [
        "Load the workflow's instructions before calling any of its MCP tools",
        "Never add `repair`, `rebuild`, `--yes`, or an id the user did not give.",
        "read `references/<workflow>.md` beside this SKILL.md and follow it to the end",
        "Unclear**: ask one question",
    ],
    "forget": [
        "cannot be undone",
        "delete <id>",
        "Always confirm with the user before calling forget",
    ],
    "distill": [
        "rebuild <page-id>",
        "force=true",
        "user-edited page prose is wiped",
    ],
    "curate": [
        "revision_source_id",
        "Perform no mutation until the user replies",
        "Ambiguous replies do not mutate",
    ],
    "lint": [
        "Population truncation is honest coverage metadata, not an automatic incomplete result.",
        "every packet candidate has exactly one accepted verdict",
        "trust the typed report's `complete` flag and preserve its denominator, evaluated, and truncation metadata",
        "Lint creates durable Review Items for choices that are not yet exact.",
        "turns exactly one Review Item into a separately approved manifest",
        "`lint_repair_review` generic accept remains rejected and non-mutating",
        "Never call `lint_repair_apply` in the same turn as `lint_repair_prepare`.",
        "lint_repair_prepare",
        "Lead repair output with exactly one compact typed-count funnel",
        "Never substitute check, family, or candidate counts for occurrence counts",
        "one or more exact approval lines",
        "contain only ready tuples from the same displayed plan",
        "no duplicates, blank lines, prose, or code fences",
        "A single line remains valid",
        "Validate the complete reply before the first apply",
        "one contiguous copy-pasteable block",
        "in the order they appear among ready tuples in the displayed plan",
        "Immediately rerun fresh General once",
        "Only after one manifest is `verified` may the next approved manifest begin",
        "`next_apply`",
        "bounded daemon handoff reservation",
        "Do not apply any later approved manifest",
        "`verified`, `applied_unverified`, `failed`, or `not_attempted`",
        "Use `applied_unverified` whenever an apply receipt exists but durable verification does not",
    ],
}
CLAUDE_ONLY_TOKENS = (
    "CLAUDE_PLUGIN_ROOT",
    ".claude-plugin",
    "mcp__plugin_wenlan_wenlan__",
)


def fail(message: str) -> None:
    print(f"codex plugin slice validation failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_json(path: Path) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing {path.relative_to(ROOT)}")
    except json.JSONDecodeError as exc:
        fail(f"{path.relative_to(ROOT)} is not valid JSON: {exc}")
    if not isinstance(payload, dict):
        fail(f"{path.relative_to(ROOT)} must contain a JSON object")
    return payload


def assert_file(path: Path) -> None:
    if not path.is_file():
        fail(f"missing {path.relative_to(ROOT)}")


def assert_no_claude_tokens(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    for token in CLAUDE_ONLY_TOKENS:
        if token in text:
            fail(f"{path.relative_to(ROOT)} contains Claude-only token {token!r}")


def shared_codex_skills() -> tuple[str, ...]:
    contract = read_json(ROOT / "plugin-contract.json")
    skills: list[str] = []
    for item in contract.get("skills", []):
        if not isinstance(item, dict):
            fail("plugin-contract.json skills entries must be objects")
        if item.get("status") == "shared_now":
            name = item.get("name")
            if not isinstance(name, str) or not name:
                fail("shared plugin-contract skill entries must have names")
            if item.get("codex_user_invocable") is not True:
                fail(f"shared skill {name} must set codex_user_invocable true")
            if item.get("codex_openai_interface") is not True:
                fail(f"shared skill {name} must set codex_openai_interface true")
            skills.append(name)
    return tuple(sorted(skills))


def validate_manifest() -> None:
    manifest = read_json(PLUGIN / ".codex-plugin" / "plugin.json")
    if manifest.get("name") != "wenlan":
        fail("plugin manifest name must stay `wenlan`")
    if "hooks" in manifest:
        fail("Codex plugin manifest must not declare hooks")
    if manifest.get("skills") != "./skills/":
        fail("plugin manifest must point skills at ./skills/")
    if manifest.get("mcpServers") != "./.mcp.json":
        fail("plugin manifest must point mcpServers at ./.mcp.json")
    interface = manifest.get("interface")
    if not isinstance(interface, dict):
        fail("plugin manifest must include interface metadata")
    prompts = interface.get("defaultPrompt")
    if not isinstance(prompts, list) or not 1 <= len(prompts) <= 3:
        fail("interface.defaultPrompt must contain 1-3 starter prompts")


def validate_mcp() -> None:
    mcp = read_json(PLUGIN / ".mcp.json")
    servers = mcp.get("mcpServers")
    if not isinstance(servers, dict):
        fail(".mcp.json must contain mcpServers object")
    wenlan = servers.get("wenlan")
    if not isinstance(wenlan, dict):
        fail(".mcp.json must define the wenlan MCP server")
    if wenlan.get("command") != "./bin/wenlan-mcp-runner.sh":
        fail("wenlan MCP server must run ./bin/wenlan-mcp-runner.sh")
    if wenlan.get("cwd") != ".":
        fail("wenlan MCP server must resolve its runner from plugin cwd")


def validate_runner() -> None:
    runner = PLUGIN / "bin" / "wenlan-mcp-runner.sh"
    assert_file(runner)
    text = runner.read_text(encoding="utf-8")
    if "--agent-name" not in text or "codex" not in text:
        fail("Codex MCP runner must pass an explicit codex agent name")


def validate_resolver() -> None:
    resolver = PLUGIN / "bin" / "resolve-space.sh"
    assert_file(resolver)
    assert_no_claude_tokens(resolver)


def hidden_codex_references() -> dict[str, str]:
    contract = read_json(ROOT / "plugin-contract.json")
    refs: dict[str, str] = {}
    for item in contract.get("skills", []):
        if isinstance(item, dict) and item.get("status") == "hidden_routed":
            name = item.get("name")
            ref = item.get("codex_reference")
            if not isinstance(name, str) or not isinstance(ref, str):
                fail("hidden_routed plugin-contract entries need name and codex_reference")
            if ref != f"wenlan/references/{name}.md":
                fail(f"hidden skill {name} must fold into wenlan/references/{name}.md")
            refs[name] = ref
    return refs


def check_content(path: Path, skill: str, text: str, *, reference: bool = False) -> None:
    normalized_text = " ".join(text.split())
    # A reference has no allowed-tools frontmatter, so it names tools bare.
    if not reference and skill not in SKILLS_WITHOUT_MCP_REFERENCE and "mcp__wenlan__" not in text:
        fail(f"{path.relative_to(ROOT)} must use Codex wenlan MCP tool names")
    if skill in SKILLS_USING_RESOLVER:
        if "plugin-codex/bin/resolve-space.sh" not in text:
            fail(f"{path.relative_to(ROOT)} must use plugin-codex/bin/resolve-space.sh")
        if RESOLVER_LOCATOR not in text:
            fail(
                f"{path.relative_to(ROOT)} must locate the installed resolver with "
                f"{RESOLVER_LOCATOR}"
            )
    for needle in REQUIRED_GUARDRAILS.get(skill, []):
        if " ".join(needle.split()) not in normalized_text:
            fail(f"{path.relative_to(ROOT)} must contain guardrail {needle!r}")


def validate_references() -> None:
    refs = hidden_codex_references()
    if set(refs) != REQUIRED_REFERENCES:
        fail(f"REQUIRED_REFERENCES must exactly match hidden_routed skills: {sorted(refs)}")
    for skill, ref in sorted(refs.items()):
        path = PLUGIN / "skills" / ref
        assert_file(path)
        assert_no_claude_tokens(path)
        if (PLUGIN / "skills" / skill).exists():
            fail(f"{(PLUGIN / 'skills' / skill).relative_to(ROOT)} must not exist; {skill} is a wenlan reference")
        text = path.read_text(encoding="utf-8")
        if text.startswith("---"):
            fail(f"{path.relative_to(ROOT)} is a reference file and must not carry frontmatter")
        check_content(path, skill, text, reference=True)
    stray = sorted(
        p.relative_to(ROOT).as_posix()
        for p in (PLUGIN / "skills" / "wenlan" / "references").rglob("*")
        if p.is_file() and p.name not in {f"{s}.md" for s in refs}
    )
    if stray:
        fail(f"unexpected files under the wenlan references: {stray}")


def validate_skills() -> None:
    required_skills = shared_codex_skills()
    if set(REQUIRED_SKILL_INTERFACE) != set(required_skills):
        fail(
            "REQUIRED_SKILL_INTERFACE must exactly match shared Codex skills: "
            f"{sorted(required_skills)}"
        )
    present = sorted(p.name for p in (PLUGIN / "skills").iterdir() if p.is_dir())
    if present != list(required_skills):
        fail(f"plugin-codex/skills must hold exactly the shared skills {list(required_skills)}, found {present}")
    for skill in required_skills:
        path = PLUGIN / "skills" / skill / "SKILL.md"
        metadata_path = PLUGIN / "skills" / skill / "agents" / "openai.yaml"
        assert_file(path)
        assert_file(metadata_path)
        assert_no_claude_tokens(path)
        assert_no_claude_tokens(metadata_path)
        text = path.read_text(encoding="utf-8")
        if f"name: {skill}" not in text:
            fail(f"{path.relative_to(ROOT)} frontmatter must name {skill}")
        if "user-invocable: true" not in text:
            fail(f"{path.relative_to(ROOT)} must be marked user-invocable for slash autocomplete")
        check_content(path, skill, text)
        metadata = metadata_path.read_text(encoding="utf-8")
        expected = REQUIRED_SKILL_INTERFACE[skill]
        if "interface:" not in metadata:
            fail(f"{metadata_path.relative_to(ROOT)} must declare Codex app interface metadata")
        for key, value in expected.items():
            expected_line = f'  {key}: "{value}"'
            if expected_line not in metadata:
                fail(f"{metadata_path.relative_to(ROOT)} must contain {expected_line}")


def validate_pages_skill() -> None:
    path = PLUGIN / "skills" / "wenlan" / "references" / "pages.md"
    text = path.read_text(encoding="utf-8")
    required = [
        "wenlan pages",
        "Do not read a page body unless the user asks to see it here",
        "Do not use a picker",
        "If several pages match, print the CLI output",
        "If `wenlan` is not found, tell the user to run `$wenlan`",
    ]
    for needle in required:
        if needle not in text:
            fail(f"{path.relative_to(ROOT)} must contain {needle!r}")
    forbidden = [
        "AskUserQuestion",
        "native picker",
        "command sandbox DISABLED",
        "mcp__plugin_wenlan_wenlan__",
    ]
    for needle in forbidden:
        if needle in text:
            fail(f"{path.relative_to(ROOT)} must not contain {needle!r}")


def main() -> None:
    validate_manifest()
    validate_mcp()
    validate_runner()
    validate_resolver()
    validate_skills()
    validate_references()
    validate_pages_skill()
    print("Codex plugin slice validation passed")


if __name__ == "__main__":
    main()
