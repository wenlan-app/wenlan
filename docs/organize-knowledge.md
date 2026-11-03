# Organizing existing knowledge

`/wenlan organize <request>` follows the complete request across accessible
Spaces. It inventories and reads existing material before making scoped changes.
It does not require a separate plan confirmation for routine organization.

```text
User request + inspected existing material
                  |
                  +-- Memory: new confirmed fact -> capture once
                  |           correction -> supersedes; pending stays pending
                  |
                  +-- Pages: needed content / plan / navigation -> update or create
                  |          reuse IDs, preserve sources, check expected version
                  |
                  +-- Folders: useful grouping -> create and move existing pages
                  |            keep page content, ID and Space
                  |
                  +-- Topics/entities: established named identity -> reuse or create
                  |                    folder names alone are not entities
                  |
                  +-- Relations: explicit fact + current source -> safe create
                              same fact -> unchanged; conflict -> protected repair
                  |
                  v
Read back content, resolved links, locations and semantic state
Report applied / pending / blocked / unchecked; skip unchanged work on reruns
```

The canonical behavioral contract is
[`plugin/skills/organize/SKILL.md`](../plugin/skills/organize/SKILL.md).
Claude, Codex, host-neutral agents and the desktop skill bundle derive from it.
The Standard MCP profile adds folder inventory/creation, page movement, resolved
page links, entity inventory and entity detail. Movement and folder creation are
local stdio operations. A locked connection cannot create unowned folders;
unsupported placement is reported without widening access.

A folder hierarchy is navigation, not evidence of a semantic relationship.
`create_relation` uses the dedicated `/api/memory/relations/preserve` endpoint:
older daemons fail closed. Under the edge-write transaction, the source must
remain current, the predicate must already be recognized, conflicting active
edges cannot be replaced, and retired edges cannot be revived. A same-triple
retry returns the original edge without replacing evidence. Responses distinguish
`created` from `unchanged`. Existing internal extraction behavior is unchanged.
Explicit corrections use the existing Deep Review Item / prepared repair path;
that path retains its exact later-turn approval, rather than adding a confirmation
to every organization run. Without a matching finding, correction is blocked.

Other workflows' pages retain their body unless the user specifically asks to
change them. Page lifecycle is separate from publication state and measured
outcomes. Plans remain plans, deleted posts remain deleted, and unknown results
remain unknown. Same-title pages in different Spaces use qualified links whose
resolved IDs are checked; ambiguous same-Space targets remain blocked.

## Acceptance recipe

Build candidate `wenlan-server` and `wenlan-mcp`. Start a daemon with an isolated
port, `WENLAN_DATA_DIR` and configured page root; set `WENLAN_NO_AUTOSTART=1`.
Verify `/api/health` reports that data root and `/api/knowledge/path` reports its
`pages` directory before seeding. Never target the installed library.

Create `ORGANIZE_SYNTHETIC_FIXTURE` in the scratch root, then run:

```sh
python3 scripts/organize-acceptance.py seed \
  --binary <candidate-wenlan-mcp> --origin <scratch-loopback-origin> --root <scratch-root>
```

Give a fresh agent the resulting `request.txt`, candidate skills and official MCP
only. Do not give it repository context, shell, filesystem or raw HTTP access.
Run `verify` with the same arguments afterward. Inspect the resulting prose;
this driver checks structure and preservation, not the quality of a plan.
Run the identical request in a second fresh session and then `verify-rerun`.
Check actual tool events for extra memory/entity writes as well as page stability.

Render the real app root with `vite.preview.config.ts` and
`WENLAN_PREVIEW_DAEMON=<scratch-origin>` (not the fixture `/preview/` route).
Navigate through the folder view to the homepage, its resolved brand link, and
the platform plan. Inspect the page content and screenshots.

## Observed synthetic run

Two fresh Claude Sonnet sessions, high effort, completed through ACP with only
candidate Wenlan MCP tool calls. The first made 42 calls: it retained all 11
existing pages, captured the new positioning once, updated the matching brand
and home pages, created one substantive three-platform plan, and moved seven
existing pages plus the new plan. Readback preserved original page IDs, Spaces,
source references, and the body/version of other workflows and unrelated pages.
Cross-Space links resolved to the intended IDs despite a same-title distractor.
The second session made 24 read calls and no mutations; all 12 page IDs, contents
and versions remained unchanged. These are one synthetic scenario, not a claim
that every possible library or model run is covered.

The lead inspected the generated plan: it contains shared preparation and
separate Reddit, Show HN and Product Hunt action lists, with unresolved product,
community and timing decisions stated. It does not claim publication or results.
The real frontend navigation completed in Playwright Chromium headless shell
149.0.7827.55 at 1440 x 1100 with no page errors. Native Tauri shell integration
and platform publishing were not exercised.

The MCP/shared-type package suite passed 703 tests; the 76 focused core relation
tests and 17 reader-manifest tests also passed. A causal
check disabled the new transaction guard: the conflicting-create regression
failed by observing an unexpected successful write; restoring the guard returned
the relation suite to green.

Automated coverage additionally exercises stale/colliding moves, strict Space
scope, entity reads, relation creation/retry/conflicts, source currency,
retired-edge protection and old-daemon refusal. Required cross-platform/full
workspace checks remain owned by the repository CI planner.
