# Reorganize existing knowledge around the user's goal

Use this when the user asks to structure, rewrite, connect, classify, or
otherwise improve existing knowledge. The request itself is authorization for
the scoped changes it describes; do not add a plan-approval round. Preserve its
full goals, constraints, named topics, and audience. Apply only the relevant
changes and verify them.

## Inspect the requested scope

Honor an explicit `space:<name>` exactly. If none is named, inspect all Spaces
the connection can access; a Space lock remains authoritative. Don't guess an
inaccessible Space or silently narrow scope.

Inventory `list_pages` for each requested Space and each status (`active`,
`draft`, `archived`), paginating with `limit=200` and increasing `offset` until
a page returns fewer than 200. Record stable page ID, Space, status,
`storage_path`, `creation_kind`, and review/source metadata. Also call
`list_folders` in the same scope. Search `recall` for the goal and explicit
decisions; its results are ranked, so no hit does not prove a memory is absent.
Read the body of every page you may change, reuse, or rely on using
`read_page(page_id=...)`.

For central names, use targeted `list_entities` queries and read matching
entities by stable ID. Before capturing a new durable fact, search for an
existing memory and revision; if none exists, capture one complete statement
faithful to what the user established and retain its returned ID as the
source. Don't infer facts or create duplicates.

If a required MCP capability is missing on the connected Wenlan version,
stop that portion and report the compatibility limit. Do not substitute raw
HTTP, shell scripts, direct database access, or an installed upgrade.

## Decide and apply changes

Use only identities and facts established by the user or supported by
inspected sources. Preserve each page's Space; a folder or Space alone does
not establish ownership. Leave pages clearly managed by another workflow
alone unless the user explicitly asks to edit them, then retain that
workflow's protections.

For a request to arrange material by platform, project, or area, prefer a small
set of useful navigation folders together with a linked entry page. Derive the
folders from inspected material and move the existing matching pages; a list of
links alone does not complete a requested grouping. Do not create empty category
pages or a universal taxonomy. For a content-only request, no folder move is needed.

Choose the mix the goal calls for:

- **Memories:** capture a new confirmed fact once. For an explicit correction,
  use `capture` with the existing ID as `supersedes` and respect its pending
  review result. A pending correction is not current yet.
- **Pages:** reuse matching pages by identity and body, not title alone. A
  useful entry page, substantive plan, or other page may be created or updated
  when it serves the request. Keep a plan separate from its execution record.
  Pass existing IDs, latest `expected_version`, and all existing
  `source_memory_id` values unchanged; append only new IDs used. Include
  stable page IDs and read versions in `basis` when content relies on those
  pages. Re-read on version conflicts. Never present planned work as applied.
- **Folders:** create only useful folders grounded in the material. Under a
  Space lock, do not create unowned folders; reuse an existing folder or
  report blocked. Move active pages with their current `page_id` and
  `storage_path`; preserve ID, body, version, sources, and Space. Draft and
  archived page moves are blocked. Use a unique `operation_id` for a retry
  of the same operation. Re-read after conflicts and skip unchanged moves.
- **Topics and entities:** create a named person, project, or topic only when
  the user established it durably and no matching entity exists. A folder
  name is not an entity.
- **Relations:** create only explicit, source-backed durable relationships.
  Read existing relations first. `create_relation` preserves the existing
  graph: a repeated triple stays unchanged and a conflicting relation
  refuses. Never try another relation type to force replacement.

If an explicit correction needs an old relation retired, use the lint repair
workflow and require fresh Deep evidence plus its matching Review Item. Prepare
only the exact `entity_relation` choice (`add` or `retire`). If no current
finding matches, leave the relation unchanged and report blocked. The
workflow displays the exact approval line and requires approval in a later
turn; do not apply it in this run. After an approved repair, inspect current
relations and dependent pages. Superseding a memory alone does not prove its
graph relation was removed.

Pages clearly managed by another workflow keep their body unless the user
specifically asks to change them. For stale pages, use the
[refresh-pages operation](refresh-pages.md), not an unguarded page write.

## Verify links and outcomes

Put entry-page navigation in ordinary Markdown paragraphs or bullet lists.
The editor's table preview displays link labels without usable link targets;
do not make table cells the only route to a page. Existing navigation tables
need a normal linked list when completing this request, even if API link
resolution already succeeds. Tables can still hold comparisons.

Wikilinks resolve by title, not page ID. Use `[[Title]]` only for a unique
same-Space title. For a cross-Space target, use `[[Space/Title]]`. A duplicate
same-Space title has no safe ID-target link: leave it unlinked and report the
ambiguity. Verify every written link with `get_page_links` and compare its
resolved `target_page_id` with the inventory. Correct and recheck only when
an unambiguous qualified title exists; otherwise remove the unsafe link and
report it blocked.

Keep Wenlan page states (`active`, `draft`, `archived`) distinct from social
content states. Report social outcomes as measured only when inspected
material establishes them; otherwise unknown. A deleted post remains deleted;
a plan stays planned until verified as executed.

After edits, re-list affected statuses and folders, re-read every changed
page, and inspect current entity relations when relevant. Report concise
**applied**, **pending**, **blocked**, and **unchecked** groups. Include an
entry page's location as `Space / title`; report ambiguity and unknown
outcomes plainly. A successful write alone is not readback evidence.
