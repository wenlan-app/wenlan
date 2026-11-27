# Knowledge guide: sources, memories, pages, and search

This guide explains how Wenlan connects source material, memories, pages, graph relationships, search, updates, review, and local files.

Languages: [English](knowledge-guide.md) · [繁體中文](knowledge-guide.zh-Hant.md) · [简体中文](knowledge-guide.zh-Hans.md) · [Español](knowledge-guide.es-ES.md)<br>
[Back to the English README](../README.md)

Contents: [Sources and pages](#sources-and-pages) · [Graph and search](#graph-and-search) · [Updates and history](#updates-and-history)

<a id="sources-and-pages"></a>

## Sources and pages

The example adapts [agent-design guidance](https://www.anthropic.com/engineering/building-effective-agents) and [this README's diagram-review fixes](https://github.com/wenlan-app/wenlan/commit/061fbc6ab12a76ec805869e46d464a58a06db296) into a working note. It illustrates reusable knowledge, not a measured customer outcome or an automatically generated page.

Wenlan gives ongoing work a place outside the chat window. Keep selected documents and conversations, save decisions made along the way, and organize them into pages you can read, edit, and reuse. You can author a page directly with your connected AI; optional model-backed synthesis and background maintenance require a [configured model](setup-and-data.md#models).

<a id="what-wenlan-is-not"></a>

**Built for work that continues.** If you use AI on the same topic over days or weeks and keep hunting for earlier material or re-explaining decisions, this is the workflow Wenlan is built around. It is not a life-management system or a memory SDK embedded inside another product. You can keep using Obsidian; Wenlan does not promise to replace its plugins or migrate every vault feature.

**One knowledge system, three roles:**

- **Sources keep the material Wenlan reads traceable.** Imported conversations remain as captured records; registered files sync their current contents as they change.
- **Memories preserve what work teaches you.** Agents capture atomic decisions, lessons, corrections, and supersession with provenance.
- **Pages compile current knowledge.** Wenlan turns relevant Sources and Memories into source-cited Markdown you can reuse, refresh, and review.

**How updates work:** Sources and captured Memories can both support the same Page. Memory history records changes to individual Memories; Page history records the supporting evidence and revisions. During automatic refresh, eligible machine-maintained Pages can update directly, while Pages you have edited receive proposed revisions. Review lets you decide whether to apply an update; it is not a guarantee that the AI's conclusion is correct.

For technical readers: Wenlan follows the **LLM wiki** pattern. See the [implementation guide](https://wenlan.app/learn/distilled-wiki-pages-ai-memory) and [technical foundations](technical-foundations.md) for the data model, retrieval, and maintenance rules.

<a id="graph-and-search"></a>

## Graph and search

The Agent Loop example connects a retry rule, a lesson from a UI review, and an acceptance checklist you can reuse.

The entity-relation graph is one part of Wenlan's wider set of connected pages. **Knowledge Pages** hold maintained synthesis, **Entities** anchor reusable people, projects, and concepts, **Source Pages** make imported or synchronized material inspectable, and atomic **Memories** preserve decisions and changes. They work through separate, explicit links: Page-to-Page wikilinks, Page evidence, Memory-to-Entity links, and directed Entity relations.

Within the entity graph, a configured enrichment model extracts typed Entities, observations, and directed relations from Memories. Entity linking and resolution reuse existing nodes instead of treating every mention as new; each Memory keeps its Source and can link to multiple Entities. [How the connected model is stored ->](technical-foundations.md#connected-knowledge-model)

- **Meaning and direction:** Relations use a seeded vocabulary such as `uses`, `part_of`, `contradicts`, and `replaced_by`; unknown types fall back to `related_to` and become reviewable vocabulary proposals.
- **Strength and provenance:** A relation can store confidence, an explanation, and its source Memory, so stronger and weaker claims remain distinguishable and inspectable.
- **Communities that compound:** Label propagation groups Entities by relation density, weighted by the relation count between each pair. These groups can organize optional corpus summaries while Entity links add retrieval context.
- **Correction without erasure:** Related claims, corrections, and explicit supersession stay inspectable together while original Sources and Memory history remain.

During retrieval, dense entity matching finds query-relevant entities. When eligible graph links exist, the default graph-memory stream boosts linked Memories as a third [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf) signal. The path is data- and scope-dependent, and Space boundaries still apply. [How the graph path works ->](technical-foundations.md#graph-assisted-retrieval)

<a id="retrieval"></a>

### Retrieval across words, meaning, and connections

Wenlan's core search is a local hybrid pipeline, not a single vector lookup. Each stage has a different job:

- **Exact wording, [SQLite FTS5](https://www.sqlite.org/fts5.html):** a full-text index finds literal terms, identifiers, and phrases.
- **Similar meaning, FastEmbed + [`Qdrant/bge-base-en-v1.5-onnx-Q`](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q):** a quantized English model creates 768-dimensional embeddings; [libSQL cosine DiskANN](https://turso.tech/blog/approximate-nearest-neighbor-search-with-diskann-in-libsql) indexes them for approximate nearest-neighbor retrieval.
- **Combined ranking, weighted [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf) (`k = 60`):** lexical and semantic rank lists are fused without pretending their raw scores share a scale; cosine similarity also weights the vector contribution.
- **Connected context, graph-memory stream:** eligible entity links add a third RRF signal while the active read scope still filters returned Memories.
- **Optional precision, cross-encoder reranking:** unlike embeddings, [`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en) or [`BAAI/bge-reranker-base`](https://huggingface.co/BAAI/bge-reranker-base) reads each query-candidate pair and reorders the smaller pool; reranking is off by default.

Page, episodic, and fact channels are opt-in and degrade to the remaining search signals if unavailable. Space still limits the read scope. [Methods, defaults, and limitations ->](technical-foundations.md)

<a id="updates-and-history"></a>

## Updates and history

Generated pages can go stale; a memory store can fragment into disconnected facts. Wenlan links two lifecycles without collapsing them into one layer.

### Atomic Memory

`CAPTURE -> CLASSIFY -> ENRICH -> LINK -> RECONCILE`

Capture and explicit supersession are core. Model-backed stages run only when the matching model is configured, and the reconcile pass is off by default.

| Operation | What Wenlan does |
|---|---|
| **Capture** | Agents write one complete, self-contained idea per Memory, following the Zettelkasten atomic-note principle instead of saving the whole conversation. |
| **Classify** | With a configured language model, Wenlan assigns `identity`, `preference`, `decision`, `lesson`, `gotcha`, or `fact`; a precise type supplied by the caller remains authoritative. |
| **Enrich** | With a configured language model, adds structured fields, retrieval cues, event dates, quality, importance, and tags when available. |
| **Link** | Retains provenance and, when enrichment is enabled, connects Memories to entities and relations in the knowledge graph. |
| **Reconcile** | Explicit replacements preserve a `supersedes` chain. A replacement from an agent whose trust level is below full queues for human review automatically, no flag required. An optional model-backed pass can also queue protected conflicts for review instead of overwriting history; that pass is off by default and must be explicitly enabled. |

Advanced configuration: set `WENLAN_ENABLE_DUAL_POOL_RESOLVE=1` to enable that reconcile pass.

### Maintained Page

`DISTILL -> CITE -> TRACK -> REFRESH -> REVIEW`

| Operation | What Wenlan does |
|---|---|
| **Distill** | Compiles related Sources and Memories into one Markdown Page. |
| **Cite** | Retains citation records and verification status; automatic refresh discards a draft when its citation-support check fails. |
| **Track** | Records which evidence supports the Page, why it became stale, and a bounded changelog. |
| **Refresh** | When a Page is marked stale, rebuilds the eligible machine-maintained Page from current evidence. |
| **Review** | During automatic refresh, turns changes to a Page you edited into a proposed revision instead of a silent rewrite. |

For example, import a design document and capture a debugging decision in Codex. Wenlan can compile one Page that cites both. When that Page is automatically refreshed, it rebuilds from its current support; if you have edited it, the proposed change waits for review.

**Review scope:** this is an update policy, not a security barrier around your files. Direct file edits and the local manual-edit API do not pass through this queue. An explicit forced regeneration can also replace an edited Page; the desktop app asks for confirmation before that action.

<a id="local-markdown"></a>

### Local Markdown that works with Obsidian

Your durable synthesis remains ordinary files rather than a proprietary editor format:

- **Plain files:** Pages and session notes stay as Markdown under `~/.wenlan/`.
- **Inspectable history:** every page keeps its revisions (shown in the app, or via `get_page_revisions` over MCP), and because everything is plain files you can put `~/.wenlan` under your own version control.
- **Obsidian coexistence:** Wenlan reads an existing vault as a source. Symlink `~/.wenlan/pages/` into the vault or export a Page from the desktop app; your edits remain human-owned, and later machine refreshes become reviewable revisions.

Pages are readable without the app:

```text
$ wenlan pages "embedding work" --print
```
