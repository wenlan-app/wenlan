---
name: steep
description: >
  Keep Wenlan knowledge useful as new material arrives, its sources change,
  or the user asks to reorganize existing knowledge. Handle memories, pages,
  folders, topics and supported relationships while preserving evidence and
  review protections. Use for "steep", "沉澱", "整理我的資料", or updates
  to existing notes. Invoked through `/wenlan steep [complete request]`.
argument-hint: "[complete request]"
allowed-tools: ["Read", "Bash", "Agent", "mcp__plugin_wenlan_wenlan__steep", "mcp__plugin_wenlan_wenlan__read_page", "mcp__plugin_wenlan_wenlan__list_pages", "mcp__plugin_wenlan_wenlan__write_page", "mcp__plugin_wenlan_wenlan__recall", "mcp__plugin_wenlan_wenlan__list_folders", "mcp__plugin_wenlan_wenlan__create_folder", "mcp__plugin_wenlan_wenlan__move_page", "mcp__plugin_wenlan_wenlan__get_page_links", "mcp__plugin_wenlan_wenlan__list_entities", "mcp__plugin_wenlan_wenlan__read_entity", "mcp__plugin_wenlan_wenlan__capture", "mcp__plugin_wenlan_wenlan__create_entity", "mcp__plugin_wenlan_wenlan__create_relation", "mcp__plugin_wenlan_wenlan__lint", "mcp__plugin_wenlan_wenlan__lint_repair_prepare", "mcp__plugin_wenlan_wenlan__lint_repair_apply", "mcp__plugin_wenlan_wenlan__lint_repair_verify", "mcp__plugin_wenlan_wenlan__distill", "mcp__plugin_wenlan_wenlan__list_refinements", "mcp__plugin_wenlan_wenlan__accept_refinement", "mcp__plugin_wenlan_wenlan__reject_refinement"]
user-invocable: false
---

# Steep: keep knowledge useful

Steep (沉澱) is Wenlan's complete knowledge-organizing process. It prepares
memories for retrieval, turns supported material into useful pages, keeps
pages aligned with changed evidence, and can reorganize existing knowledge
when the user names a goal. It can update memories, pages, folders, topics,
and evidence-backed relationships as the request requires.

Preserve the user's full goal, scope, constraints, named topics, and intended
audience. Do not invent facts, ownership, or outcomes. Keep source links,
stable identities, history, and review protections. Do not ask for a plan
approval before ordinary requested organization. If a requested change needs
a protected repair or destructive rebuild, follow that operation's specific
approval rule.

## Choose only the work this request needs

For an explicit reorganization goal with no named Space, skip the project
resolver and search across accessible Spaces within the reshape reference's bounds.
For ordinary catch-up, new material, or source refresh, resolve the Space once
from the full arguments using the same project resolver as other workflows:

```bash
raw_args="<the full argument string passed to /wenlan steep>"
resolved="$("${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
space="$(printf '%s\n' "$resolved" | cut -f1)"
```

An explicit Space or configured lock takes precedence; otherwise this keeps
the current project context. When the user explicitly asks to reorganize
existing knowledge and gives no Space, the reshape operation instead inspects
accessible Spaces, as described in its reference.

For catch-up or an explicit reorganization goal, first call
`steep(action="list", limit=100, space=...)` once in the requested scope.
Use its `new_total` and `total` to choose the references below: load
`new-material.md` only when `new_total > 0`, and `refresh-pages.md` only when
`total > 0`. For a goal, apply the reshape reference's relevance filter before
loading either maintenance reference; counts alone do not authorize unrelated
work. If both are zero, report no queued work. An explicit
reorganization goal still loads `reshape-knowledge.md` and runs even when
both counts are zero. An explicit one-page rebuild loads `rebuild-pages.md`;
don't substitute a global pass. Pass the worklist into the chosen references
instead of listing it again.

- New or imported material needs pages or to be connected to existing
  knowledge: read [new-material.md](references/new-material.md).
- A source, memory, or cited code changed and dependent pages need review:
  read [refresh-pages.md](references/refresh-pages.md).
- The user gives a goal to reorganize existing knowledge, whether or not there
  is queued work: read [reshape-knowledge.md](references/reshape-knowledge.md).
- The user explicitly asks to rebuild one page, review a specific page merge
  proposal, or uses legacy `distill <page>`: read
  [rebuild-pages.md](references/rebuild-pages.md).

Requests can need more than one operation. Read each relevant reference, then
do the work in an order that preserves its evidence and protections. Do not
load every reference by default. A plain request to organize or tidy existing
knowledge is a goal for this workflow; do not route it to a separate organize
or distill workflow.

## Automatic and on-demand work

Wenlan may run eligible enrichment and evidence-based page maintenance in the
background when configured. On demand, this AI tool can process new material
and waiting page updates now. Background processing does not infer a user's
broader reorganization goal; perform that only when the user asks.

Resolve and honor the requested Space. An explicit Space lock takes precedence;
for explicit reshaping without a named Space, search across accessible Spaces.
For ordinary catch-up, retain the resolved project scope. If a necessary tool is unavailable, stop that portion and say
what could not be done. Never use raw HTTP, shell scripts, or direct database
access as a substitute for the supported Wenlan tools.

## Session and final report

When the session start printed a `Wenlan session:` line, pass its value as
`session` on every steep commit and write.

When making a memory capture or calling `write_page` from reshape, pass that
same `session` value there too.

Read back material changes before reporting them. Separate **applied**,
**pending**, **blocked**, and **unchecked** work; don't describe a proposal or
plan as completed. Skip unchanged work on reruns. Reply in the user's language;
call the workflow “steep” in English and “沉澱” in Chinese.
