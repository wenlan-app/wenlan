# Steep: knowledge that stays useful

[English](steep.md) · [繁體中文](steep.zh-Hant.md) · [Back to README](../README.md#keep-knowledge-current-and-preserve-history)

Steep (沉澱) is Wenlan’s process for keeping knowledge useful. It takes new material, changes to existing sources, and a goal you give it, then updates the affected memories, pages, folders, topics, and relationships when needed. It keeps sources, review protections, and history attached so later work can build on what you learned.

There is one user-facing workflow: `/wenlan steep` (or `$wenlan steep` in Codex). Say what you want to organize after `/wenlan`; the front door routes the full request to this workflow. Steep inspects the current material and acts on the relevant parts. An explicit reorganization request can run even when no new material or stale page is queued. It does not ask for a fixed plan confirmation before every run.

```text
New material / changed source / your organization goal
                         |
                         v
                    Steep (沉澱)
                         |
       +-- Memories: add a confirmed finding or preserve a correction
       +-- Pages: create, enrich, refresh, merge, or rebuild when needed
       +-- Folders: improve grouping and navigation when useful
       +-- Topics: add or update a clearly supported subject
       `-- Relations: add source-backed links; protect conflicts for review
                         |
                         v
              Keep sources, review rules, and history
```

On demand, the connected AI reads the relevant Steep procedure and carries out the request through Wenlan's MCP tools. The AI decides what the evidence supports; the daemon owns queues, claim tokens, guarded writes, and the single-writer path. Some steps can use a small AI helper for bounded enrichment or judgment. Automatic Steep is different: the daemon handles eligible background work with the models configured for those jobs. It does not dispatch work to arbitrary connected AI tools or infer a free-form organization goal. See [when work runs](#when-it-runs) and [how to invoke Steep](#enable-steep-and-check-its-work).

The short Steep entry routes to one of four focused procedures: [new material](../plugin/skills/steep/references/new-material.md), [source changes](../plugin/skills/steep/references/refresh-pages.md), [a reorganization goal](../plugin/skills/steep/references/reshape-knowledge.md), or [an explicit page rebuild](../plugin/skills/steep/references/rebuild-pages.md). Read only the procedure needed for the request. Page synthesis, clustering, merging, and rebuild remain internal operations; `/wenlan distill` is not a separate workflow.

## Memory lifecycle: preserve and refine individual findings

A memory holds one decision, lesson, preference, or fact. It can be retrieved on its own; it does not need to become a page first.

| Stage | What changes |
|---|---|
| **Capture** | Save a self-contained finding with its available source context. |
| **Classify and enrich** | With the assigned model, add a type, structured details, and retrieval cues. An explicit type supplied by the caller remains authoritative. |
| **Link** | With enrichment enabled, match people, projects, and concepts by alias, name, or similarity, connecting memories from different material to the same entity while retaining their sources. |
| **Revise or reconcile** | An explicit replacement links the new memory to the earlier one through `supersedes`. The optional model-backed conflict-reconciliation pass is off by default. |

A newer document does not automatically replace an earlier memory. Once a replacement takes effect, its mode determines whether the earlier memory is hidden from ordinary retrieval or remains visible as archived; the replacement link is retained. See the [memory review rules](knowledge-guide.md#atomic-memory).

## Page lifecycle: maintain an editable synthesis

A page combines related sources and memories into cited Markdown that you can read and edit. You can write it in the App, ask your AI to write it, or let Steep synthesize accumulated material when that work is needed. No separate command is required to create a page.

| Stage | What changes |
|---|---|
| **Write or synthesize** | Save a page, or let configured page synthesis organize eligible related material. Not every import creates a page. |
| **Cite and track** | Record the sources and memories supporting the page, so changes to that evidence can mark it for refresh. |
| **Refresh** | Rebuild an eligible page from current support and check its citations. |
| **Apply or review** | Update an AI-maintained page directly, or stage a revision when its ownership or review lock requires it; the exact rules are below. |
| **Keep history** | Retain a bounded change history for the page, separate from the replacement links between memories. |

**New findings can grow an existing page.** With background processing and its assigned model enabled, Wenlan matches an eligible new memory to a generated page in the same Space, using a shared entity or semantic similarity. It combines the page’s existing support with the new memory to update the content and citations. This automatic growth skips pages you created or edited, and review-locked pages; no suitable match means no page update from this step.

### How evidence drives a page update

Wenlan records which sources and memories support a page. When a tracked item changes in Wenlan, dependent pages are marked for refresh and their previous citation-check results are cleared. An external file change must first reach Wenlan through its source refresh path.

With background processing and the assigned model available, refresh builds a draft from current support and checks its numbered citations. A rejected draft leaves the saved page intact; a passing draft follows the ownership and review rules below. One changed item can affect several pages, each handled separately. Sources and memories remain directly retrievable without a page.

The check compares cited sentences, or their enclosing paragraphs, with supplied source text. It is a text-overlap check, not a factual or logical proof, and does not check every uncited statement. [Citation-check scope and limits](citation-checks.md).

## When it runs

| Event | What happens |
|---|---|
| Material is imported or a memory is captured | Eligible unfinished work enters background processing: classification, extraction, linking, and configured synthesis. |
| A cited source or memory is changed, replaced, or removed in Wenlan | Dependent pages are marked for refresh. For external files, Wenlan must first read the change through the source’s refresh path. |
| Background work is enabled and its assigned model is available | The local service processes eligible work in bounded turns, subject to resource and scheduling limits. Saving does not imply an immediate refresh. |
| Background work is off or its model is unavailable | That model-backed work does not run; existing material remains available for reading, editing, and retrieval. |

The background-organization and page-synthesis tasks have separate model assignments. Configuring one does not guarantee that the other can run.

## Automatic update or review?

These are the rules for **background page refresh**, not every possible edit or goal-driven organization request:

| Page state | Result |
|---|---|
| AI-maintained, not edited by you, with no review lock | A valid refresh can update the page directly. |
| Created or edited by you | The proposed change waits as a pending revision; the current page stays in place. |
| Review lock enabled | Machine edits wait for approval, including explicit AI edits. |
| Draft fails the citation-support check | The draft is not applied; the saved page stays unchanged. |

A pending revision applies to its named page, not every page touched by the same source. You can inspect the change and accept or skip it. History remains available to inspect earlier changes.

## Organize existing knowledge around a goal

When you ask Steep to reorganize existing knowledge, it reads the request and relevant current material, then updates only what the goal calls for:

- Add a confirmed new finding once. Preserve corrections through the existing `supersedes` behavior; a pending correction remains pending.
- Update or create page content, plans, and navigation while preserving page IDs, sources, Space, and version checks. A plan stays a plan; Steep does not claim an action happened without evidence.
- Create or move pages into folders when that helps navigation. Folder names alone do not establish a topic or semantic relationship.
- Reuse or create a topic/entity when the material supports a clear identity.
- Add relationships only from current source evidence. Same facts remain unchanged; a conflicting edge is protected for the existing repair and review path.
- Read back the changed content, resolved links, locations, and semantic state. Report applied, pending, blocked, or unchecked work, and skip unchanged work on a rerun.

For an explicit goal, Steep checks its pending-work list at the start and refreshes it after evidence changes, then handles or reports relevant items within a bounded scope; unrelated backlog is outside the request. It can discover matching material in other accessible Spaces when the goal calls for it, using targeted, bounded reads rather than requiring a full crawl of every status. No separate plan approval is required for a routine request. Destructive deletion and protected repairs keep their existing controls.

Folder creation and page movement in the Standard MCP profile are local stdio operations. Folder creation is rejected for every scoped connection, including a Space-locked one; use an unscoped local connection when a new folder is needed. The `create_relation` preserve operation uses `/api/memory/relations/preserve`: the source must still be current, predicates must be recognized, active conflicts are refused, retired edges cannot be revived, and a same-triple retry leaves the original evidence intact. Older daemons fail closed. These are write-path protections; they do not turn folder placement into evidence of a semantic relation.

Other workflows' pages keep their content unless the user asks to change it. Ambiguous same-Space page targets stay blocked; same-title pages in different Spaces use qualified links whose resolved IDs are checked. Page state does not establish publication or measured outcomes.

**Explicit edits differ:** a connected AI that reads the current page and submits a matching version can update it directly unless review is locked. Your manual edits do not enter this queue. An explicitly forced rebuild can replace an edited generated page; authored pages still require review. That rebuild is available inside Steep when you directly request it, but it is not the default for ordinary steeping. [Full update and history reference](knowledge-guide.md#updates-and-history).

<a id="enable-steep"></a>

## Enable Steep and check its work

In Settings, under **Steeping**, choose a mode:

- **On demand** (new installs start here) needs no background model. Your AI tool handles a requested organization goal and can work through new or stale material. Run `/wenlan steep` (`$wenlan steep` in Codex) to catch up on eligible new material and pages awaiting refresh.
- **Automatic** processes eligible background enrichment, synthesis, and page refresh while the computer is idle. Configure an AI service and assign it to background organization and/or page synthesis. It does not perform an unrequested goal-driven reorganization.

The model check of each sentence against its source runs only in Automatic for now; in On demand, pages show “Not checked”.

For Automatic, choose on-device Qwen through llama.cpp, an OpenAI-compatible local server such as Ollama or LM Studio, or a supported cloud provider. A remote provider receives the material needed for its tasks. [Model choices and privacy](setup-and-data.md#models).

From the CLI:

```bash
wenlan steep configure --help  # Choose the task-specific model sources
wenlan steep status           # Inspect each task’s off, ready, or paused state
wenlan steep disable          # Stop model-backed background work; keep providers
```

Use Activity to check processing status, and the affected page’s revision and history views to inspect changes. If a page has not updated, check whether its source change has reached Wenlan, its assigned model is ready, or a revision is waiting for review.

## Why prepare knowledge ahead of time?

Reusable memories and cited pages can reduce repeated context gathering and rereading of originals. Local models can handle background tasks without per-request cloud API charges. Results depend on the material, model, and task; Steep does not guarantee lower total cost, faster responses, or more accurate answers.

Implementation references: [background scheduling](../crates/wenlan-server/src/scheduler.rs), [new-memory page growth](../crates/wenlan-core/src/post_ingest.rs), [entity matching](../crates/wenlan-core/src/post_write/entity_graph.rs), [page synthesis and refresh](../crates/wenlan-core/src/synthesis/distill.rs), [page ownership and review rules](../crates/wenlan-core/src/post_write/page_update.rs).

## Isolated workflow acceptance

The synthetic organization fixture can be exercised without touching a real
library. Start a scratch daemon with a separate port, data root, and pages
directory; create the fixture marker in that root; then use
[`scripts/organize-acceptance.py`](../scripts/organize-acceptance.py) with the
`seed`, `verify`, and `verify-rerun` actions around a cold agent run. The driver
refuses the default production port and verifies the daemon's live pages path
before it reads or writes. Its checks cover retained page identity, Space and
sources, expected content and navigation, unrelated pages, and an unchanged
rerun. It is a fixture/readback check, not a substitute for exercising the
connected AI, UI, native Tauri app, or automatic background model path; those
need their own evidence.

Add `--with-maintenance` when seeding to put a relevant authored page and an
unrelated page in the same Space's stale queue. Verification requires a pending
revision for the relevant page, unchanged original bodies, and no revision for
the unrelated page; a rerun must preserve the pending revision too.
