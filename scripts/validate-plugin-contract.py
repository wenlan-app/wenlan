#!/usr/bin/env python3
"""Validate the shared Wenlan Claude/Codex plugin contract.

The contract is deliberately small: it documents which skills are shared now,
which are hidden behind the /wenlan entry (Codex reference files), which Claude
skills are not ported yet, and the surface-specific MCP, runner,
and marketplace rules that should not drift silently.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any


DEFAULT_ROOT = Path(__file__).resolve().parents[1]
ALLOWED_SKILL_STATUSES = {"shared_now", "claude_only_until_ported", "hidden_routed"}
CODEX_SKILLS_WITHOUT_MCP_REFERENCE = {"handoff", "pages"}
CODEX_SKILLS_USING_RESOLVER = {
    "brief",
    "capture",
    "distill",
    "handoff",
    "lint",
    "map",
    "recall",
}
# Installed plugins live under ~/.codex/plugins/cache/<marketplace>/wenlan/<version>/;
# the checkout-relative `plugin-codex/bin/...` path only resolves inside this repo.
CODEX_RESOLVER_LOCATOR = (
    "find \"$HOME/.codex/plugins/cache\" -path '*/wenlan/*/bin/resolve-space.sh'"
)
CODEX_REQUIRED_GUARDRAILS = {
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
}
LINT_GRAMMAR = "[deep|repair] [global|uncategorized|space:<name>]"
LINT_SHARED_GUARDRAILS = [
    "{entry} lint repair",
    "General uses exactly one lint MCP call",
    "Agent-assisted Deep uses exactly two lint MCP calls",
    "never evaluate records outside agent_work",
    "`work_digest`",
    "do not retry: immutable plan artifacts may already exist",
    "submit verdicts exactly once",
    "authorized record refs (`evidence_refs` plus `counterevidence_refs`)",
    "Do not mechanically copy every evidence ref",
    "Population truncation is honest coverage metadata, not an automatic incomplete result.",
    "every packet candidate has exactly one accepted verdict",
    "trust the typed report's `complete` flag and preserve its denominator, evaluated, and truncation metadata",
    "If Deep is incomplete, its producer receipt differs from General, or its DB analysis digest differs from General, rerun fresh General exactly once after Deep before prepare; do not rerun Deep.",
    "Do not compare General and Deep Page digests across profiles because their Page scan coverage intentionally differs.",
    "Lead repair output with exactly one compact typed-count funnel",
    "Never substitute check, family, or candidate counts for occurrence counts",
    "CLI fallback: `wenlan lint --profile deep --agent-assist`",
    "global",
    "uncategorized",
    "Plain `{entry} lint`, `{entry} lint deep`, the lint MCP tool, and `/api/lint` are fully read-only",
    "Lint creates durable Review Items for choices that are not yet exact.",
    "turns exactly one Review Item into a separately approved manifest",
    "`lint_repair_review` generic accept remains rejected and non-mutating",
    "Never call `lint_repair_apply` in the same turn as `lint_repair_prepare`.",
    "lint_repair_prepare",
    "the exact `plan_id` and `plan_digest`",
    "every observed family",
    "`ready`",
    "`review`",
    "`system_action`",
    "`blocked`",
    "artifact_path",
    "canonical data is unchanged",
    "apply repair <manifest-id> <manifest-digest>",
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
    "Never call lint_repair_apply in the same turn as lint_repair_prepare, whether",
    "applied_unverified",
    "Match every line byte-for-byte",
    "the MCP repair-manifest tools have no CLI equivalent yet",
]
# A skill that refreshes pages reads a page's `basis` first and settles a
# pending one through `page_basis`; the note-id/memory-id split must stay stated.
PAGE_BASIS_SKILLS = {"handoff", "distill"}
PAGE_BASIS_GUARDRAILS = [
    "`pending`",
    '"diff"',
    '"confirm"',
    "`basis`",
    "`archived`, `deleted` or `unavailable`",
]
PAGES_SHARED_GUARDRAILS = [
    'Resolve the page ID first with `wenlan pages "<query-or-filename>" --resolve-id`.',
]
# The front door must keep the workflow protections in force: it loads the
# chosen workflow before any of that workflow's tools, and never invents a mode
# or approval the user did not type.
WENLAN_ENTRY_GUARDRAILS = [
    "Load the workflow's instructions before calling any of its MCP tools",
    "Pass the user's mode, scope, target and approval words through unchanged.",
    "Never add `repair`, `rebuild`, `--yes`, or an id the user did not give.",
    "Unclear**: ask one question with two or three options. Do not guess.",
    "lint repair needs a later turn and an exact approval",
    "Import progress only when the user asks about an import",
]
ENRICHMENT_CONSENT_GUARDRAILS = [
    "wenlan steep status",
    "wenlan steep configure --everyday <source> --synthesis <source>",
    "wenlan steep disable",
]


def validate_capture_relation_flow(
    root: Path,
    surface: str,
    skill_path: Path,
    expected_prefix: str,
    text: str,
) -> None:
    # Both surfaces name the tools bare in prose; the Codex skill is generated
    # from the Claude one, so the wording is identical.
    def tool(name: str) -> str:
        return name

    steps = [
        (
            f"1. Call `{tool('create_entity')}` for both named endpoints first "
            "and collect their stable ids."
        ),
        (
            f"2. Call `{tool('capture')}` with the complete relation statement "
            "and pass the primary entity name as `entity` so the memory resolves "
            "and links to it."
        ),
        (
            f"3. Call `{tool('create_relation')}` with `from_entity_id`, "
            "`to_entity_id`, `relation_type`, and the capture result's required "
            "`source_memory_id`."
        ),
    ]
    normalized_text = " ".join(text.split())
    positions = []
    for step in steps:
        position = normalized_text.find(step)
        if position < 0:
            fail(
                f"{rel(root, skill_path)} must contain ordered relation step "
                f"{step!r}"
            )
        positions.append(position)
    if positions != sorted(positions):
        fail(
            f"{rel(root, skill_path)} must resolve entities, capture the linked "
            "memory, then create the relation"
        )
    policies = [
        "only when the user explicitly states a durable relation",
        f"Do not call `{tool('create_entity')}` for ordinary captures.",
        "Never infer a relation the user did not state.",
    ]
    for policy in policies:
        if policy not in normalized_text:
            fail(
                f"{rel(root, skill_path)} must contain relation policy "
                f"{policy!r}"
            )


def validate_session_brief_flow(
    root: Path,
    _surface: str,
    skill_path: Path,
    expected_prefix: str,
    frontmatter: dict[str, str],
    text: str,
) -> None:
    normalized = " ".join(text.split())
    forbidden = [
        "context(",
        "Call FIRST at session start",
        "BEFORE any other Wenlan",
        "cat ~/.wenlan/sessions/_status",
    ]
    for needle in forbidden:
        if needle in text:
            fail(f"{rel(root, skill_path)} contains retired Brief flow {needle!r}")
    if re.search(r"cat[^\n]*sessions/_status", text):
        fail(f"{rel(root, skill_path)} must not read the Markdown status receipt")

    if frontmatter.get("name") == "brief":
        expected_allowed = {
            "Bash",
            f"{expected_prefix}brief",
            f"{expected_prefix}list_pending_revisions",
            f"{expected_prefix}accept_revision",
            f"{expected_prefix}dismiss_revision",
        }
        allowed: Any = None
        try:
            allowed = json.loads(frontmatter.get("allowed-tools", ""))
        except json.JSONDecodeError:
            fail(f"{rel(root, skill_path)} allowed-tools must be a JSON array")
        if not isinstance(allowed, list) or set(allowed) != expected_allowed:
            fail(
                f"{rel(root, skill_path)} allowed-tools must be exactly "
                f"{sorted(expected_allowed)!r}"
            )
        required = [
            "No topic means the complete Brief alone.",
            "Related Context",
            "Brief reads never create state.",
            "It is not a mandatory every-session boot step.",
            "one-way human-readable receipt",
        ]
    else:
        required = [
            "Read the Brief before composing deltas",
            "This read is mandatory before any Brief delta is authored for a registered Space.",
            "brief update --file",
            "Never fuzzy-match",
            "Never auto-demote",
            "one-way human receipt",
            "Never read, edit, or overwrite that receipt as authority.",
            "cwd-repo-new",
            "For `cwd-repo-new`, prove the Space is absent with `spaces show` before composing deltas.",
            "Outside a Git repository, do not derive a new Space from the directory basename.",
            "Apply the Brief update before Space-scoped captures when this fallback is new.",
            "Every delta for one existing item uses the same version from the pre-handoff Brief snapshot.",
        ]
        retired_handoff = [
            "status_json=",
            "Promotion / demotion rules",
            "Overwrite `~/.wenlan/sessions/_status",
            "handoff-<project>.json",
        ]
        for needle in retired_handoff:
            if needle in text:
                fail(f"{rel(root, skill_path)} contains retired handoff state {needle!r}")

    for needle in required:
        if needle not in normalized:
            fail(f"{rel(root, skill_path)} must contain Brief guardrail {needle!r}")


def fail(message: str) -> None:
    print(f"plugin contract validation failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def rel(root: Path, path: Path) -> str:
    try:
        return str(path.relative_to(root))
    except ValueError:
        return str(path)


def read_json(root: Path, path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing {rel(root, path)}")
    except json.JSONDecodeError as exc:
        fail(f"{rel(root, path)} is not valid JSON: {exc}")
    if not isinstance(payload, dict):
        fail(f"{rel(root, path)} must contain a JSON object")
    return payload


def read_text(root: Path, path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"missing {rel(root, path)}")


def require_file(root: Path, path: Path) -> None:
    if not path.is_file():
        fail(f"missing {rel(root, path)}")


def require_equal(label: str, actual: Any, expected: Any) -> None:
    if actual != expected:
        fail(f"{label} must be {expected!r}, got {actual!r}")


def load_contract(root: Path) -> dict[str, Any]:
    contract = read_json(root, root / "plugin-contract.json")
    require_equal("plugin-contract.json schema_version", contract.get("schema_version"), 1)
    surfaces = contract.get("surfaces")
    if not isinstance(surfaces, dict):
        fail("plugin-contract.json must contain surfaces object")
    for surface in ("claude", "codex"):
        if not isinstance(surfaces.get(surface), dict):
            fail(f"plugin-contract.json surfaces must include {surface}")
    skills = contract.get("skills")
    if not isinstance(skills, list) or not skills:
        fail("plugin-contract.json must contain non-empty skills array")
    return contract


def contract_skill_sets(
    contract: dict[str, Any],
) -> tuple[set[str], set[str], dict[str, str]]:
    names: set[str] = set()
    shared_now: set[str] = set()
    claude_only: set[str] = set()
    hidden: dict[str, str] = {}
    for item in contract["skills"]:
        if not isinstance(item, dict):
            fail("each plugin-contract skill entry must be an object")
        name = item.get("name")
        status = item.get("status")
        if not isinstance(name, str) or not name:
            fail("each plugin-contract skill entry must have a name")
        if name in names:
            fail(f"duplicate plugin-contract skill entry {name!r}")
        names.add(name)
        if status not in ALLOWED_SKILL_STATUSES:
            fail(f"skill {name} has unsupported status {status!r}")
        if status == "shared_now":
            if item.get("codex_user_invocable") is not True:
                fail(f"shared skill {name} must set codex_user_invocable true")
            if item.get("codex_openai_interface") is not True:
                fail(f"shared skill {name} must set codex_openai_interface true")
            shared_now.add(name)
        elif status == "claude_only_until_ported":
            claude_only.add(name)
            if not item.get("reason"):
                fail(f"claude-only skill {name} must document why it is not ported yet")
        elif status == "hidden_routed":
            # Hidden from the Claude menu and reached through /wenlan; in Codex
            # it is a reference file under the wenlan skill.
            ref = item.get("codex_reference")
            if ref != f"wenlan/references/{name}.md":
                fail(f"hidden skill {name} must set codex_reference 'wenlan/references/{name}.md'")
            hidden[name] = ref
    if hidden and "wenlan" not in shared_now:
        fail("hidden_routed skills need the shared wenlan entry skill")
    return shared_now, claude_only, hidden


def skill_names(root: Path, plugin_root: Path) -> set[str]:
    skills_root = plugin_root / "skills"
    if not skills_root.is_dir():
        fail(f"missing {rel(root, skills_root)}")
    return {
        path.parent.name
        for path in skills_root.glob("*/SKILL.md")
        if path.is_file()
    }


def parse_frontmatter(root: Path, path: Path) -> dict[str, str]:
    text = read_text(root, path)
    lines = text.splitlines()
    if not lines or lines[0] != "---":
        fail(f"{rel(root, path)} must start with frontmatter")
    metadata: dict[str, str] = {}
    for line in lines[1:]:
        if line == "---":
            return metadata
        match = re.match(r"^([A-Za-z0-9_-]+):\s*(.*)$", line)
        if match:
            metadata[match.group(1)] = match.group(2).strip().strip('"')
    fail(f"{rel(root, path)} frontmatter is not closed")


def validate_manifest(root: Path, surface: str, config: dict[str, Any]) -> None:
    manifest_path = root / config["manifest_path"]
    manifest = read_json(root, manifest_path)
    require_equal(f"{rel(root, manifest_path)} name", manifest.get("name"), config["manifest_name"])
    if "manifest_category" in config:
        require_equal(
            f"{rel(root, manifest_path)} category",
            manifest.get("category"),
            config["manifest_category"],
        )
    if "manifest_display_name" in config:
        require_equal(
            f"{rel(root, manifest_path)} displayName",
            manifest.get("displayName"),
            config["manifest_display_name"],
        )
    if surface == "claude" and "category" in manifest:
        # `category` belongs to marketplace.json; in plugin.json the Claude CLI
        # validator warns and the directory portal requires a clean validate.
        fail(f"{rel(root, manifest_path)} must not declare category (marketplace.json owns it)")
    if surface == "codex":
        require_equal(
            f"{rel(root, manifest_path)} skills",
            manifest.get("skills"),
            config["manifest_skills"],
        )
        require_equal(
            f"{rel(root, manifest_path)} mcpServers",
            manifest.get("mcpServers"),
            config["manifest_mcp_servers"],
        )
        if "hooks" in manifest:
            fail(f"{rel(root, manifest_path)} must not declare hooks")


def validate_mcp_config(root: Path, surface: str, config: dict[str, Any]) -> None:
    path = root / config["mcp_config_path"]
    mcp = read_json(root, path)
    servers = mcp.get("mcpServers")
    if not isinstance(servers, dict):
        fail(f"{rel(root, path)} must contain mcpServers object")
    server_name = config["mcp_server_name"]
    server = servers.get(server_name)
    if not isinstance(server, dict):
        fail(f"{rel(root, path)} must define MCP server {server_name!r}")
    require_equal(
        f"{surface} MCP command",
        server.get("command"),
        config["mcp_command"],
    )
    if "mcp_cwd" in config:
        require_equal(
            f"{surface} MCP cwd",
            server.get("cwd"),
            config["mcp_cwd"],
        )


def validate_runner(root: Path, surface: str, config: dict[str, Any]) -> None:
    runner_path = root / config["runner_path"]
    runner = read_text(root, runner_path)
    agent = config["agent_name"]
    if agent["mode"] == "runner_argument_default":
        env = agent["env"]
        value = agent["value"]
        if f'{env}:-{value}' not in runner:
            fail(f"{rel(root, runner_path)} must default {env} to {value}")
        if '--agent-name "${agent_name}"' not in runner:
            fail(f"{rel(root, runner_path)} must pass --agent-name through the runner")
    elif agent["mode"] == "wenlan_mcp_stdio_default":
        if "--agent-name" in runner:
            fail(f"{rel(root, runner_path)} must rely on the wenlan-mcp stdio default agent")
        source = read_text(root, root / "crates" / "wenlan-mcp" / "src" / "main.rs")
        pattern = re.compile(
            r"if\s+serve_args\.is_some\(\)\s*\{.*?\"remote-mcp\"\.into\(\).*?\}"
            r"\s*else\s*\{.*?\"" + re.escape(agent["value"]) + r"\"\.into\(\).*?\}",
            re.DOTALL,
        )
        if not pattern.search(source):
            fail(f"wenlan-mcp stdio default agent must remain {agent['value']!r}")
    else:
        fail(f"{surface} has unsupported agent mode {agent['mode']!r}")


SHARED_PLUGIN_SCRIPTS = (
    ("scripts/resolve-space.sh", "bin/resolve-space.sh"),
    ("scripts/version-drift.sh", "bin/version-drift.sh"),
    ("scripts/test/test-resolve-space.sh", "bin/test/test-resolve-space.sh"),
    ("scripts/test/test-version-drift.sh", "bin/test/test-version-drift.sh"),
)


def validate_resolver_parity(root: Path) -> None:
    # The Codex plugin ships byte-identical copies of the Claude plugin's shell
    # helpers under bin/; a fix landing on one side only is a contract break.
    for claude_rel, codex_rel in SHARED_PLUGIN_SCRIPTS:
        claude_copy = read_text(root, root / "plugin" / claude_rel)
        codex_copy = read_text(root, root / "plugin-codex" / codex_rel)
        if claude_copy != codex_copy:
            fail(f"plugin-codex/{codex_rel} must match plugin/{claude_rel}")


def validate_codex_readme(root: Path) -> None:
    path = root / "plugin-codex" / "README.md"
    text = read_text(root, path)
    if "$wenlan" not in text:
        fail(f"{rel(root, path)} must direct users to $wenlan")
    if re.search(r"(?<![A-Za-z0-9_-])/init\b", text):
        fail(f"{rel(root, path)} must not advertise the retired /init command")


def iter_matching_plugin(plugins: list[Any], plugin_name: str):
    for item in plugins:
        if isinstance(item, dict) and item.get("name") == plugin_name:
            yield item


def validate_marketplace(root: Path, surface: str, config: dict[str, Any]) -> None:
    marketplace_config = config.get("marketplace")
    if not isinstance(marketplace_config, dict):
        fail(f"{surface} contract must include marketplace object")
    path = root / marketplace_config["path"]
    marketplace = read_json(root, path)
    require_equal(f"{rel(root, path)} name", marketplace.get("name"), marketplace_config["name"])
    plugins = marketplace.get("plugins")
    if not isinstance(plugins, list):
        fail(f"{rel(root, path)} must contain plugins array")
    plugin = next(iter_matching_plugin(plugins, marketplace_config["plugin_name"]), None)
    if plugin is None:
        fail(f"{rel(root, path)} must contain plugin {marketplace_config['plugin_name']!r}")
    label = f"{surface.capitalize()} marketplace"
    source = plugin.get("source")
    if not isinstance(source, dict):
        fail(f"{rel(root, path)} plugin source must be an object")
    require_equal(f"{label} source", source.get("source"), marketplace_config["source"])
    require_equal(
        f"{label} source.path",
        source.get("path"),
        marketplace_config["source_path"],
    )
    if "source_url" in marketplace_config:
        require_equal(
            f"{label} source.url",
            source.get("url"),
            marketplace_config["source_url"],
        )
    if "policy_installation" in marketplace_config:
        policy = plugin.get("policy")
        if not isinstance(policy, dict):
            fail(f"{rel(root, path)} plugin policy must be an object")
        require_equal(
            f"{label} policy.installation",
            policy.get("installation"),
            marketplace_config["policy_installation"],
        )
        require_equal(
            f"{label} policy.authentication",
            policy.get("authentication"),
            marketplace_config["policy_authentication"],
        )
    require_equal(
        f"{label} category",
        plugin.get("category"),
        marketplace_config["category"],
    )
    # The storefront listing is not a second copy to keep in sync by hand: the
    # blurb and the discovery tags must be the manifest's own.
    manifest = read_json(root, root / config["manifest_path"])
    for field in ("description", "keywords"):
        if field in plugin:
            require_equal(
                f"{label} {field}",
                plugin.get(field),
                manifest.get(field),
            )


def validate_skill_text(
    root: Path,
    surface: str,
    skill_path: Path,
    name: str,
    frontmatter: dict[str, str] | None,
    text: str,
    expected_prefix: str,
) -> None:
    """Workflow guardrails that must survive on both surfaces. `frontmatter` is
    None for a Codex reference file, which carries no frontmatter."""
    entry = "/wenlan" if surface == "claude" else "$wenlan"
    if name in {"brief", "handoff"}:
        validate_session_brief_flow(
            root, surface, skill_path, expected_prefix, frontmatter, text
        )
    if name == "capture":
        validate_capture_relation_flow(
            root,
            surface,
            skill_path,
            expected_prefix,
            text,
        )
    if name == "curate" and frontmatter is not None:
        require_equal(
            f"{rel(root, skill_path)} argument-hint",
            frontmatter.get("argument-hint"),
            "captures | revisions | refinements",
        )
        try:
            allowed_tools = json.loads(frontmatter.get("allowed-tools", ""))
        except json.JSONDecodeError:
            fail(f"{rel(root, skill_path)} allowed-tools must be a JSON array")
        expected_allowed_tools = {
            "Bash",
            f"{expected_prefix}list_pending",
            f"{expected_prefix}confirm_memory",
            f"{expected_prefix}forget",
            f"{expected_prefix}capture",
            f"{expected_prefix}recall",
            f"{expected_prefix}list_refinements",
            f"{expected_prefix}accept_refinement",
            f"{expected_prefix}reject_refinement",
        }
        if surface == "claude":
            expected_allowed_tools.add("AskUserQuestion")
        if (
            not isinstance(allowed_tools, list)
            or len(allowed_tools) != len(expected_allowed_tools)
            or set(allowed_tools) != expected_allowed_tools
        ):
            fail(
                f"{rel(root, skill_path)} allowed-tools must be exactly "
                f"{sorted(expected_allowed_tools)!r}"
            )
    if name == "curate":
        normalized_text = " ".join(text.split())
        lint_repair_guardrail = (
            "A generic accept does not apply `lint_repair_review`; route that "
            f"action through `{entry} lint repair` instead."
        )
        guardrails = [
            (
                f"Use `{entry} curate refinements` only when the user explicitly asks "
                "to inspect or review the daemon proposal/refinement queue."
            ),
            "Never poll the refinement queue ambiently.",
            (
                f"List first with `{expected_prefix}list_refinements(limit=50)` "
                "and show at most four items."
            ),
            (
                "Perform no mutation until the user gives an unambiguous "
                "item-level accept or reject decision."
            ),
            "Skip or cancel is a no-op.",
            "Re-list after every mutation batch.",
            "`vocab_promote`",
            lint_repair_guardrail,
        ]
        for guardrail in guardrails:
            if guardrail not in normalized_text:
                fail(
                    f"{rel(root, skill_path)} must contain refinement guardrail "
                    f"{guardrail!r}"
                )
    if name == "lint" and frontmatter is not None:
        require_equal(
            f"{rel(root, skill_path)} argument-hint",
            frontmatter.get("argument-hint"),
            LINT_GRAMMAR,
        )
    if name == "lint":
        for tool in (
            "lint",
            "lint_repair_prepare",
            "lint_repair_apply",
            "lint_repair_verify",
        ):
            expected_tool = f"{expected_prefix}{tool}"
            if expected_tool not in text:
                fail(f"{rel(root, skill_path)} must call {expected_tool!r}")
    if name == "lint" and frontmatter is not None:
        try:
            allowed_tools = json.loads(frontmatter.get("allowed-tools", ""))
        except json.JSONDecodeError:
            fail(f"{rel(root, skill_path)} allowed-tools must be a JSON array")
        expected_allowed_tools = {
            "Bash",
            *(f"{expected_prefix}{tool}" for tool in (
                "lint",
                "lint_repair_prepare",
                "lint_repair_apply",
                "lint_repair_verify",
            )),
        }
        if not isinstance(allowed_tools, list) or set(allowed_tools) != expected_allowed_tools:
            fail(
                f"{rel(root, skill_path)} allowed-tools must be exactly "
                f"{sorted(expected_allowed_tools)!r}"
            )
    if name == "lint":
        normalized_text = " ".join(text.split())
        for needle in LINT_SHARED_GUARDRAILS:
            needle = needle.format(entry=entry)
            if needle not in normalized_text:
                fail(f"{rel(root, skill_path)} must contain guardrail {needle!r}")
        resolver = (
            "$CLAUDE_PLUGIN_ROOT/scripts/resolve-space.sh"
            if surface == "claude"
            else "plugin-codex/bin/resolve-space.sh"
        )
        if resolver not in text:
            fail(f"{rel(root, skill_path)} must use {resolver}")
    if name in PAGE_BASIS_SKILLS:
        normalized_text = " ".join(text.split())
        if frontmatter is not None:
            try:
                skill_tools = json.loads(frontmatter.get("allowed-tools", ""))
            except json.JSONDecodeError:
                fail(f"{rel(root, skill_path)} allowed-tools must be a JSON array")
            for tool_name in ("read_page", "page_basis"):
                if f"{expected_prefix}{tool_name}" not in skill_tools:
                    fail(
                        f"{rel(root, skill_path)} allowed-tools must include "
                        f"{expected_prefix}{tool_name}"
                    )
        for needle in PAGE_BASIS_GUARDRAILS:
            if needle not in normalized_text:
                fail(f"{rel(root, skill_path)} must contain page basis guardrail {needle!r}")
    if name == "pages":
        normalized_text = " ".join(text.split())
        for needle in PAGES_SHARED_GUARDRAILS:
            if needle not in normalized_text:
                fail(f"{rel(root, skill_path)} must contain guardrail {needle!r}")
    if name == "wenlan":
        normalized_text = " ".join(text.split())
        for needle in WENLAN_ENTRY_GUARDRAILS:
            if needle not in normalized_text:
                fail(f"{rel(root, skill_path)} must contain entry guardrail {needle!r}")
    if name in {"wenlan", "setup"}:
        normalized_text = " ".join(text.split())
        for needle in ENRICHMENT_CONSENT_GUARDRAILS:
            if needle not in normalized_text:
                fail(
                    f"{rel(root, skill_path)} must delegate background consent through {needle!r}"
                )


def validate_skill_surface(
    root: Path,
    surface: str,
    config: dict[str, Any],
    expected_names: set[str],
    shared_now: set[str],
    contract: dict[str, Any],
    hidden: dict[str, str],
) -> None:
    plugin_root = root / config["plugin_root"]
    actual_names = skill_names(root, plugin_root)
    if "lint-repair" in actual_names or "lint-repair" in expected_names:
        fail(f"{surface} must expose repair only through the lint skill")
    if actual_names != expected_names:
        fail(
            f"{surface} skill inventory drift: expected {sorted(expected_names)}, "
            f"got {sorted(actual_names)}"
        )

    expected_prefix = config["skill_mcp_tool_prefix"]
    other_prefix = (
        contract["surfaces"]["codex"]["skill_mcp_tool_prefix"]
        if surface == "claude"
        else contract["surfaces"]["claude"]["skill_mcp_tool_prefix"]
    )

    for name in sorted(expected_names):
        skill_path = plugin_root / "skills" / name / "SKILL.md"
        require_file(root, skill_path)
        frontmatter = parse_frontmatter(root, skill_path)
        require_equal(f"{rel(root, skill_path)} frontmatter name", frontmatter.get("name"), name)
        text = read_text(root, skill_path)
        if other_prefix in text:
            fail(f"{rel(root, skill_path)} contains wrong MCP tool prefix {other_prefix!r}")
        mcp_tools = set(re.findall(r"mcp__[A-Za-z0-9_]+__[A-Za-z0-9_]+", text))
        wrong_tools = sorted(token for token in mcp_tools if not token.startswith(expected_prefix))
        if wrong_tools:
            fail(f"{rel(root, skill_path)} contains unexpected MCP tools: {wrong_tools}")
        if mcp_tools and not any(token.startswith(expected_prefix) for token in mcp_tools):
            fail(f"{rel(root, skill_path)} must use MCP prefix {expected_prefix!r}")
        validate_skill_text(root, surface, skill_path, name, frontmatter, text, expected_prefix)
        hidden_flag = frontmatter.get("user-invocable")
        if surface == "claude":
            if name in hidden:
                require_equal(f"{rel(root, skill_path)} user-invocable", hidden_flag, "false")
            elif hidden_flag is not None:
                fail(f"{rel(root, skill_path)} must not set user-invocable; only hidden_routed skills do")

        if surface == "codex" and name in shared_now:
            require_equal(
                f"{rel(root, skill_path)} user-invocable",
                frontmatter.get("user-invocable"),
                "true",
            )
            if name not in CODEX_SKILLS_WITHOUT_MCP_REFERENCE and expected_prefix not in text:
                fail(f"{rel(root, skill_path)} must use MCP prefix {expected_prefix!r}")
            if name in CODEX_SKILLS_USING_RESOLVER:
                if "plugin-codex/bin/resolve-space.sh" not in text:
                    fail(f"{rel(root, skill_path)} must use plugin-codex/bin/resolve-space.sh")
                if CODEX_RESOLVER_LOCATOR not in text:
                    fail(
                        f"{rel(root, skill_path)} must locate the installed resolver with "
                        f"{CODEX_RESOLVER_LOCATOR}"
                    )
            for needle in CODEX_REQUIRED_GUARDRAILS.get(name, []):
                if needle not in text:
                    fail(f"{rel(root, skill_path)} must contain guardrail {needle!r}")
            metadata_path = plugin_root / "skills" / name / "agents" / "openai.yaml"
            require_file(root, metadata_path)
            metadata = read_text(root, metadata_path)
            if "interface:" not in metadata:
                fail(f"{rel(root, metadata_path)} must declare interface metadata")

    if surface != "codex":
        return
    for name, ref in sorted(hidden.items()):
        ref_path = plugin_root / "skills" / ref
        require_file(root, ref_path)
        text = read_text(root, ref_path)
        if text.startswith("---"):
            fail(f"{rel(root, ref_path)} is a reference file and must not carry frontmatter")
        if (ref_path.parent / "agents").exists() or (ref_path.parent.parent / name).exists():
            fail(f"{name} must exist in Codex only as {rel(root, ref_path)}")
        if other_prefix in text:
            fail(f"{rel(root, ref_path)} contains wrong MCP tool prefix {other_prefix!r}")
        validate_skill_text(root, surface, ref_path, name, None, text, expected_prefix)
        if name in CODEX_SKILLS_USING_RESOLVER:
            if "plugin-codex/bin/resolve-space.sh" not in text or CODEX_RESOLVER_LOCATOR not in text:
                fail(f"{rel(root, ref_path)} must locate the installed resolver with {CODEX_RESOLVER_LOCATOR}")
        for needle in CODEX_REQUIRED_GUARDRAILS.get(name, []):
            if needle not in text:
                fail(f"{rel(root, ref_path)} must contain guardrail {needle!r}")


GENERATOR = Path(__file__).resolve().parent / "gen-codex-plugin.mjs"


def validate_codex_generated(root: Path) -> None:
    """Every Codex skill must equal what gen-codex-plugin.mjs produces from the
    Claude skill plus plugin-codex/overlay.json. A hand edit to a generated file,
    an overlay `from` run that no longer matches the Claude text, or a leftover
    Claude-only token all fail here."""
    node = shutil.which("node")
    if node is None:
        fail("node is required to check the generated Codex skills (scripts/gen-codex-plugin.mjs)")
    result = subprocess.run(
        [node, str(GENERATOR), "--check", "--root", str(root)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        fail(f"Codex skills are not what the generator produces:\n{detail}")


def validate_contract(root: Path) -> None:
    contract = load_contract(root)
    shared_now, claude_only, hidden = contract_skill_sets(contract)
    surfaces = contract["surfaces"]

    for surface, config in surfaces.items():
        validate_manifest(root, surface, config)
        validate_mcp_config(root, surface, config)
        validate_runner(root, surface, config)

    validate_resolver_parity(root)
    validate_codex_generated(root)
    validate_codex_readme(root)
    validate_marketplace(root, "claude", surfaces["claude"])
    validate_marketplace(root, "codex", surfaces["codex"])

    validate_skill_surface(
        root,
        "claude",
        surfaces["claude"],
        shared_now | claude_only | set(hidden),
        shared_now,
        contract,
        hidden,
    )
    validate_skill_surface(
        root,
        "codex",
        surfaces["codex"],
        shared_now,
        shared_now,
        contract,
        hidden,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    args = parser.parse_args()
    validate_contract(args.root.resolve())
    print("Plugin contract validation passed")


if __name__ == "__main__":
    main()
