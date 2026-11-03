# /wenlan organize

Organize existing knowledge around the user's complete request. Keep their
goals, constraints, named topics, and intended audience intact. Reply in their
language. The user has authorized this scoped, non-destructive organizing; do
not ask for a plan approval or permission for each step.

## 1. Scope and inspect

Parse an explicit `space:<name>` from the complete request and pass that exact
name to the tools. If none is named, omit `space` and inspect all Spaces this
connection can access; a locked Space remains authoritative. Do not guess an
inaccessible Space or silently narrow the user's scope.

Inventory `list_pages` for every requested Space, with `status` set to each of
`active`, `draft`, and `archived`. Paginate using `limit=200` and increasing
`offset` until a page returns fewer than 200. When no Space was specified, omit
`space` so all accessible Spaces are included. Record each page's stable
`page_id`, Space, status, `storage_path`, `creation_kind`, and review/source
metadata. Also call `list_folders` for the same scope.

Use `recall` for a few targeted searches for the user's goal and explicit
decisions. Recall is ranked, not exhaustive; never treat its absence of a hit
as proof that no memory exists. For a new durable fact the user establishes
during this work, search first for the same fact and its existing revision. If
none exists, capture one complete statement faithful to the user's meaning and
keep the returned memory ID as its source; don't invent an interpretation or
duplicate a matching memory. For names central to the requested structure,
use `list_entities` with targeted queries and paginate if needed, then
`read_entity` by stable ID to inspect identity and current relations. Read the
actual bodies of pages that may be changed, reused as entry points, or relied on
for a factual judgment with `read_page(page_id=...)`. A title, summary, folder,
or successful write alone does not establish what a page says.

If any required new MCP tool is unavailable on the connected Wenlan version,
stop that portion, explain which capability needs compatible Wenlan versions, and
continue only independent work supported by available tools. Do not substitute
raw HTTP, shell scripts, database access, or an installed upgrade.

## 2. Decide what to organize

Use only identities and facts established by the user or supported by the
inspected pages and memories. Preserve the existing Space of every page. Don't
infer ownership from its Space or folder alone. For a page clearly managed by
another workflow (for example, a repository wiki), reuse its links and leave its
body alone unless the user explicitly asks to update that page; then follow its
own workflow's protections.

Prefer a small set of useful navigation folders and an entry page with links.
Derive folder names from this user's material; don't create empty category
pages, a universal brand tree, or categories with no matching pages. Also
create or update substantive planning pages when the request asks for them; the
scope follows the user's request, not whether a page was created by this
organizer. Identify existing entry and planning pages by identity and body, not
title alone. Reuse the matching page. If identity is unknown or duplicates are
plausible, preserve them and report the ambiguity; don't rename, merge, guess,
or ask for a planning approval.

Wikilinks resolve by title, not page ID. For a cross-Space target use the
qualified form `[[Space/Title]]`; for a same-Space target use `[[Title]]` only
when that title is unique there. Same-Space duplicate titles have no safe
ID-target wikilink form: leave the ambiguous target unlinked and report it.
After writing links, verify every one with `get_page_links` and compare each
resolved `target_page_id` to the inventory ID. If a link is unresolved or
points elsewhere, correct it only with an unambiguous Space-qualified title and
verify again; otherwise remove the link and report it blocked. Never leave a
same-title link that silently opens the wrong page.

Keep Wenlan page states (`active`, `draft`, `archived`) distinct from any social
content's draft/published/deleted state. Report social outcomes as measured
only when the inspected content says so; otherwise leave them unknown.

## 3. Apply the scoped changes

Use folder inventory to choose destinations and create only needed folders.
Under a Space lock, don't create unowned folders: reuse an existing one or
report blocked. Never remove the lock or widen access; folders don't change
Space. Move active pages with stable `page_id` and
current `storage_path`; preserve ID, body, version, sources, and Space. Reuse a
unique `operation_id` only for an identical retry. After a conflict, re-read.
Draft/archive moves are blocked.

Create or update the entry page and any requested substantive planning pages.
For each page, include links only to verified targets and pass their stable IDs
and read versions as `basis` when the content depends on them. Follow the
source workflow's protections for clearly managed pages; ownership alone is not
a reason to omit a page the user explicitly asked to update. For existing
pages, pass the stable `page_id`, latest `version` as `expected_version`, and
every existing `source_memory_id` unchanged; append newly captured IDs used on
the page. Never drop or replace an existing ID. Re-read on version conflict.
Skip writes and moves when content and location already match on a rerun.
Honor any review lock or pending change: don't bypass it, overwrite it, or
present a staged change as current. For stale dependent pages, use the
established `steep` workflow; don't overwrite them in this organizer.

Keep plans separate from execution records. A proposed or planned action stays
planned until it has actually been applied and read back; never describe a
planned result as done. Record completion only from verified changes or an
inspected source that establishes the outcome.

When the user explicitly corrects a durable memory, use `capture` with the
existing memory ID as `supersedes`, following capture's pending-review result.
If it is pending, it is not current yet. An accepted memory revision does not
guarantee that an obsolete knowledge-graph relation was removed: inspect the
entities' current relations and any dependent pages afterward.
Create an entity only when the user explicitly establishes a durable named
person, project, or topic and no existing candidate matches; folders do not
automatically become entities. Create a relation only for an explicit,
source-backed durable relationship, after reading the current relation first.
`create_relation` preserves existing relations: the same triple is unchanged,
and a conflicting relation refuses. Never retry with another relation type to
force a replacement.

When an explicit correction requires retiring a conflicting old relation, load
and follow the `/wenlan lint repair` workflow. It must use fresh Deep evidence
and a matching Review Item from that run. Prepare only its
`choice.kind="entity_relation"` choice with the exact Review Item selection
(`add` with its resolved entities, relation type, and source memory, or `retire`
with the relation ID) plus the selected finding from fresh Deep. If no matching
current Review Item and finding exist, leave the relation unchanged and report
it blocked; never fabricate either. The protected workflow displays the exact
approval line and waits for a later turn before applying. Do not apply in the
organize turn. After an approved repair, inspect current relations and dependent
pages; memory supersession alone is not evidence that a graph edge disappeared.

## 4. Verify and report

After moves and writes, re-list the affected page statuses and folder scope,
re-read every changed page, and inspect its current entity when relevant. For
every cross-Space link, call `get_page_links` on the entry page and confirm the
resolved target page IDs; a successful write is not enough.

Report concise groups of **applied**, **pending**, **blocked**, and
**unchecked** work. Include the entry location as `Space / title` so the user
can open it in Wenlan. Name unresolved duplicates or relation conflicts. Mark an
outcome unknown when no inspected source establishes it. Never report a
requested change as applied unless read-back confirms it.
