# Steep: knowledge that stays useful

[English](steep.md) · [繁體中文](steep.zh-Hant.md) · [Back to README](../README.md#keep-knowledge-current-and-preserve-history)

Steep is Wenlan’s optional background upkeep. It prepares saved material for retrieval and maintains pages from their supporting evidence, so later work can reuse what you already learned. You do not have to run a daily sequence of commands.

## What it adds

- **Memories with context.** Classification and enrichment add types, structured details, retrieval cues, and links to entities and relations. A decision can be found as an individual memory, with its supporting context, rather than by rereading a whole conversation.
- **Pages built from accumulated knowledge.** With page synthesis configured, eligible related material can become cited pages or contribute to existing ones. Not every import creates a page; sources and memories can be retrieved directly.
- **Updates that follow evidence.** When supporting material changes in Wenlan, affected pages can be refreshed from current evidence. One changed memory may support several pages; the relationship is tracked per page.

For example, a saved decision may say that passing tests is enough to ship. A later review records that mobile layouts also need checking. A replacement memory can preserve the earlier decision, while pages that cite the changed evidence become candidates for refresh. This is an illustration of the relationship, not a promise that every new document automatically replaces an earlier decision.

## When it runs

| Event | What happens |
|---|---|
| Material is imported or a memory is captured | Eligible unfinished work enters background processing: classification, extraction, linking, and configured synthesis. |
| A cited source or memory is changed, replaced, or removed in Wenlan | Dependent pages are marked for refresh. For external files, Wenlan must first read the change through the source’s refresh path. |
| Background work is enabled and its assigned model is available | The local service processes eligible work in bounded turns, subject to resource and scheduling limits. Saving does not imply an immediate refresh. |
| Background work is off or its model is unavailable | That model-backed work does not run; existing material remains available for reading, editing, and retrieval. |

The background-organization and page-synthesis tasks have separate model assignments. Configuring one does not guarantee that the other can run. Optional conflict reconciliation is off by default; Steep does not silently resolve every disagreement.

## Automatic update or review?

These are the rules for **background page refresh**, not every possible edit:

| Page state | Result |
|---|---|
| AI-maintained, not edited by you, with no review lock | A valid refresh can update the page directly. |
| Created or edited by you | The proposed change waits as a pending revision; the current page stays in place. |
| Review lock enabled | Machine edits wait for approval, including explicit AI edits. |
| Draft fails the citation-support check | The draft is not applied; the saved page stays unchanged. |

A pending revision applies to its named page, not every page touched by the same source. You can inspect the change and accept or skip it. Page history records revisions; explicit memory replacements retain a link to the earlier memory. These records help you inspect a change, but do not guarantee that an AI conclusion is correct.

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

You can still ask your connected AI to write a cited page and save it to Wenlan without first running `/distill`. Use `/distill` when you want to synthesize accumulated memories on demand.

## Why prepare knowledge ahead of time?

Reusable memories and cited pages can reduce repeated context gathering and rereading of originals. Local models can handle background tasks without per-request cloud API charges. Results depend on the material, model, and task; Steep does not guarantee lower total cost, faster responses, or more accurate answers.

Implementation references: [background scheduling](../crates/wenlan-server/src/scheduler.rs), [page refresh and citation checks](../crates/wenlan-core/src/synthesis/distill.rs), [page ownership and review rules](../crates/wenlan-core/src/post_write/page_update.rs).
