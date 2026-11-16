#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Measure the wenlan-mcp tool surface from source.

Reads `crates/wenlan-mcp/src/tools.rs`, finds every `#[tool(description = "...")]`
declaration and the standard `with_instructions("...")` string, unescapes the
Rust string literals, and prints the Unicode character count of each tool
description plus the totals. The numbers back the budget tests in `tools.rs`
(`visible_tool_descriptions_stay_within_budget`,
`instructions_open_with_what_wenlan_holds_and_stay_short`).

Usage: python3 scripts/measure-mcp-surface.py [--json]
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
TOOLS_RS = ROOT / "crates" / "wenlan-mcp" / "src" / "tools.rs"

TOOL_RE = re.compile(
    r'#\[tool\(\s*description\s*=\s*"((?:[^"\\]|\\.)*)"\s*,.*?\)\]\s*async fn (\w+)\(',
    re.S,
)
INSTRUCTIONS_RE = re.compile(r'\.with_instructions\(\s*"((?:[^"\\]|\\.)*)"', re.S)


def unescape_rust(literal: str) -> str:
    """Decode a Rust string literal body: escapes plus line continuations."""
    # A backslash at end of line swallows the newline and the next line's
    # leading whitespace.
    literal = re.sub(r"\\\n\s*", "", literal)
    out = []
    i = 0
    while i < len(literal):
        ch = literal[i]
        if ch != "\\":
            out.append(ch)
            i += 1
            continue
        nxt = literal[i + 1]
        if nxt == "n":
            out.append("\n")
        elif nxt == "t":
            out.append("\t")
        elif nxt in {'"', "\\", "'"}:
            out.append(nxt)
        elif nxt == "u":
            end = literal.index("}", i)
            out.append(chr(int(literal[i + 3 : end], 16)))
            i = end + 1
            continue
        else:
            out.append(nxt)
        i += 2
    return "".join(out)


def main() -> int:
    text = TOOLS_RS.read_text(encoding="utf-8")
    tools = [(name, len(unescape_rust(body))) for body, name in TOOL_RE.findall(text)]
    instructions = [len(unescape_rust(body)) for body in INSTRUCTIONS_RE.findall(text)]
    # The query-only block comes first in get_info; the standard block second.
    standard_instructions = instructions[-1] if instructions else 0
    total = sum(chars for _, chars in tools)
    report = {
        "tool_count": len(tools),
        "description_total_chars": total,
        "description_budget_plus_10pct": int(total * 1.1),
        "standard_instructions_chars": standard_instructions,
        "tools": {name: chars for name, chars in sorted(tools)},
    }
    if "--json" in sys.argv[1:]:
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 0
    width = max(len(name) for name, _ in tools)
    for name, chars in sorted(tools, key=lambda item: (-item[1], item[0])):
        print(f"{name:<{width}}  {chars:>6}")
    print(f"{'-' * width}  {'-' * 6}")
    print(f"{'tools':<{width}}  {len(tools):>6}")
    print(f"{'descriptions total':<{width}}  {total:>6}")
    print(f"{'budget (+10%)':<{width}}  {int(total * 1.1):>6}")
    print(f"{'standard instructions':<{width}}  {standard_instructions:>6}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
