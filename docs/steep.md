# Steep: knowledge that stays useful

[English](steep.md) · [繁體中文](steep.zh-Hant.md) · [Back to README](../README.md#keep-knowledge-current-and-preserve-history)

Steep is Wenlan’s optional background upkeep. It prepares saved material for retrieval and maintains pages from their supporting evidence, so later work can reuse what you already learned. You do not have to run a daily sequence of commands.

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

A page combines related sources and memories into cited Markdown that you can read and edit. You can write it in the App, ask your AI to write it, or synthesize accumulated material with `/wenlan distill`; `/wenlan distill` is not a prerequisite for writing a new page.

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

These are the rules for **background page refresh**, not every possible edit:

| Page state | Result |
|---|---|
| AI-maintained, not edited by you, with no review lock | A valid refresh can update the page directly. |
| Created or edited by you | The proposed change waits as a pending revision; the current page stays in place. |
| Review lock enabled | Machine edits wait for approval, including explicit AI edits. |
| Draft fails the citation-support check | The draft is not applied; the saved page stays unchanged. |

A pending revision applies to its named page, not every page touched by the same source. You can inspect the change and accept or skip it. History remains available to inspect earlier changes.

**Explicit edits differ:** a connected AI that reads the current page and submits a matching version can update it directly unless review is locked. Your manual edits do not enter this queue. An explicitly forced rebuild can replace an edited generated page; authored pages still require review. [Full update and history reference](knowledge-guide.md#updates-and-history).

<a id="enable-steep"></a>

## Enable Steep and check its work

In Settings, configure an AI service, assign it to background organization and/or page synthesis, and turn on **Background steeping**. Choose on-device Qwen through llama.cpp, an OpenAI-compatible local server such as Ollama or LM Studio, or a supported cloud provider. A remote provider receives the material needed for its tasks. [Model choices and privacy](setup-and-data.md#models).

From the CLI:

```bash
wenlan steep configure --help  # Choose the task-specific model sources
wenlan steep status           # Inspect each task’s off, ready, or paused state
wenlan steep disable          # Stop model-backed background work; keep providers
```

Use Activity to check processing status, and the affected page’s revision and history views to inspect changes. If a page has not updated, check whether its source change has reached Wenlan, its assigned model is ready, or a revision is waiting for review.

## Why prepare knowledge ahead of time?

Reusable memories and cited pages can reduce repeated context gathering and rereading of originals. Local models can handle background tasks without per-request cloud API charges. Results depend on the material, model, and task; Steep does not guarantee lower total cost, faster responses, or more accurate answers.

Implementation references: [background scheduling](../crates/wenlan-server/src/scheduler.rs), [new-memory page growth](../crates/wenlan-core/src/post_ingest.rs), [entity matching](../crates/wenlan-core/src/post_write/entity_graph.rs), [page refresh and citation checks](../crates/wenlan-core/src/synthesis/distill.rs), [page ownership and review rules](../crates/wenlan-core/src/post_write/page_update.rs).
