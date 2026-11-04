# Reorganize existing knowledge around the user's goal

Use this when the user asks to structure, rewrite, connect, classify, or
otherwise improve existing knowledge. The request itself is authorization for
the scoped changes it describes; do not add a plan-approval round. Preserve its
full goals, constraints, named topics, and audience. Apply only the relevant
changes and verify them.

## Inspect the requested scope

Honor an explicit `space:<name>` exactly. If none is named, search across Spaces
the connection can access; a Space lock remains authoritative. Don't guess an
inaccessible Space or silently narrow scope.

Start with targeted `recall` queries for the goal, named topics and decisions.
Use `list_pages(status="active", limit=200, ...)` for navigation metadata;
omit Space to search across accessible Spaces when none was requested. Read
at most 200 inventory records and 20 candidate page bodies in this run. Do not
scan every Space/status to exhaustion. Include draft or archived inventory
only when the request or inspected evidence points there, within the same
budget. Record stable IDs, Space, status, `storage_path`, `creation_kind`, and
review/source metadata; call `list_folders` in the same scope. Read every page
you may change, reuse, or rely on with `read_page(page_id=...)`.

Ranked search and bounded inventory cannot prove absence. If coverage is
incomplete, don't create a possibly duplicate page or claim the whole library
is organized. Complete the supported subset and report remaining scope and
the cursor/offset for a later pass.

For central names, use targeted `list_entities` queries and read matching
entities by stable ID. Before capturing a new durable fact, search for an
existing memory and revision; if none exists, capture one complete statement
faithful to what the user established and retain its returned ID as the
source. Don't infer facts or create duplicates.

If a required MCP capability is missing on the connected Wenlan version,
stop that portion and report the compatibility limit. Do not substitute raw
HTTP, shell scripts, direct database access, or an installed upgrade.

## Include relevant maintenance

Use the caller's initial `steep(action="list", limit=100, space=...)`; if
missing, obtain it before writes. Match waiting page IDs to the goal and
inspected pages. A nonzero total is not permission to process an unrelated
backlog, even in the same Space. Keep exclusions for other managed workflows.

If `new_total > 0`, load [new-material.md](new-material.md) to inspect one
bounded new-material batch and claim only groups relevant to this goal.
Then load [refresh-pages.md](refresh-pages.md) for relevant waiting pages.
Before directly editing an existing page, inspect its `read_page` stale state;
queued or stale content uses the guarded refresh path. If maintenance leaves
it pending, blocked or leased, preserve it and defer conflicting content edits.
Pending content review does not block a folder move. When the goal calls for
grouping, include matching active pages with pending revisions in the same
folder pass, preserving their body and revision card. Defer only moves whose
own preconditions fail or whose pages the user excluded.

Across this whole goal run, process at most eight new groups and ten refresh
pages; do not restart the budget or loop until queues are empty. Report
truncated or unhandled relevant work as pending/unchecked, not as a completed
refresh of the whole scope.

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
  Space lock, `create_folder` is unavailable; reuse an existing folder or
  report blocked. Move active pages with their current `page_id` and
  `storage_path`; preserve ID, body, version, sources, and Space. Draft and
  archived page moves are unsupported: skip them before calling `move_page`.
  That route can return NotFound for a known inactive page; do not report it
  deleted or recreate it. Choose a unique `operation_id` per intended move
  and reuse it only for an identical retry. It labels the move journal; it is
  not a server-side deduplication key. Re-read after uncertainty/conflicts,
  check the actual path, and skip already completed moves.
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

Before reporting, if this run captured, replaced or attached any source,
call `steep(action="list", limit=100, space=...)` again after the last such
change (a child operation's equally fresh list counts). Handle or report
newly relevant waiting pages within the remaining run budget; leave unrelated
work alone. The initial worklist cannot establish the final waiting state.

Then re-list affected statuses and folders, re-read every changed
page, and inspect current entity relations when relevant. Report concise
**applied**, **pending**, **blocked**, and **unchecked** groups. Include an
entry page's location as `Space / title`; report ambiguity and unknown
outcomes plainly. A successful write alone is not readback evidence.
