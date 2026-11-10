---
title: "How search works"
type: page
description: "Memory search fuses a BGE vector search and an FTS5 keyword search with weighted RRF, scales by quality signals, adds a graph boost and an optional cross-encoder; pages are looked up separately and capped at 3."
tags: ["wenlan"]
space: "wenlan"
origin_id: page_61c74305-5265-469a-b2d3-f816b5493bb1
origin_version: 1
created: 2026-10-23
modified: 2026-10-23
generated: {by: "wenlan/0.20.0+g279fad04", at: "2026-10-23T18:32:24.494883+00:00"}
status: stable
sources:
  - {resource: "repo://crates/wenlan-core/src/db.rs#L2814-L2822"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L5304-L5310"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L28316-L28348"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L28422-L28437"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L28508-L28532"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L28573-L28617"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L28683-L28724"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L28977-L29011"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L29917-L29974"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L30446-L30465"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L30582-L30602"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L30640-L30656"}
  - {resource: "repo://crates/wenlan-core/src/db.rs#L44637-L44680"}
  - {resource: "repo://crates/wenlan-core/src/pages.rs#L162-L210"}
  - {resource: "repo://crates/wenlan-core/src/read_scope.rs#L5-L56"}
  - {resource: "repo://crates/wenlan-core/src/reranker.rs#L219-L283"}
  - {resource: "repo://crates/wenlan-mcp/src/tools.rs#L2351-L2386"}
  - {resource: "repo://crates/wenlan-server/src/memory_routes.rs#L1279-L1314"}
  - {resource: "repo://crates/wenlan-server/src/memory_routes.rs#L1378-L1424"}
  - {resource: "repo://crates/wenlan-server/src/routes.rs#L242-L268"}
---

Wenlan searches memories in two ways at once. A vector search over BGE embeddings and a keyword search over SQLite FTS5 both run on the query. Their two rankings are fused, scaled by quality signals, nudged by a graph step, and can be re-ordered by a cross-encoder. Pages are looked up by a separate, smaller path and ride along beside the memories. The pipeline sits mostly in `crates/wenlan-core/src/db.rs`, with query helpers in `crates/wenlan-core/src/retrieval/`. The cross-encoder code is in `crates/wenlan-core/src/reranker.rs`, not under `retrieval/`. For the daemon see [The HTTP daemon](the-http-daemon.md), for the tool [MCP server](mcp-server.md), and for how quality is measured Retrieval evaluation.

## Who calls it

The MCP tool is `recall`. It posts the query to `POST /api/memory/search`, prints the memory results, and appends a "Compiled pages" section from the reply's `supplemental_pages`. On that route, a request with `rerank` set calls `search_memory_cross_rerank` with the deep reranker. If none is wired the daemon logs a warning and the search degrades to plain hybrid. Without `rerank` it calls `search_memory`.

`POST /api/search` is the other route. For `source_filter` "memory" it calls `search_memory`. When a light reranker is wired it first fetches a wider pool (at least 10) and cross-encodes it down to the limit. Other filters go through `db.search`.

## The memory pipeline

The path below is `search_memory` plus the optional reranker. It starts from the query and the Space. The paragraphs after the diagram explain each box.

```mermaid
flowchart LR
  q["Query and Space"] -- "embedded" --> vec["Vector search"]
  q -- "sanitized terms" --> fts["Keyword search"]
  vec --> rrf["Weighted RRF and score multipliers"]
  fts --> rrf
  rrf --> graph["Graph boost"]
  graph --> tidy["Dedup, cap, normalize"]
  tidy -- "when a reranker is on" --> ce["Cross-encoder"]
  tidy --> out["Memory results"]
  ce --> out
  q -- "separate lookup" --> pages["Page search"]
  pages --> sup["Compiled pages"]
```

**Query and Space.** The request's `space` or the Space header becomes a `ReadScope`: `Global`, `Space(name)` or `Uncategorized`. An empty value means Global, a registered Space name means that Space, and an unknown name is an error. The name "uncategorized" is ambiguous if a Space of that name exists, and that is also an error. `Uncategorized` matches rows with no Space or the unfiled id.

**Vector search.** The query is embedded with the quantized BGE-Base-EN-v1.5 model that the database loads at startup. Under Global scope the vector search uses the `memories_vec_idx` index. Under a Space or Uncategorized scope it scans `memories` ordered by cosine distance, filtered to that scope. Both channels also filter by `memory_type` (comma lists work) and by `source_agent`, and they always exclude `source = 'episode'` rows.

**Keyword search.** The query is cleaned so FTS5 operators cannot break it. It is tried as an AND match first, and if that returns nothing as an OR match over the non-stopword terms. With hardening enabled, queries over 128 tokens skip the keyword channel, so fusion becomes vector-only.

**Weighted RRF.** Each channel adds reciprocal-rank terms to a per-row score. The vector term is the similarity (1 minus cosine distance, floored at 0.01) divided by `rrf_k + rank`, with `rrf_k` 60 by default. The keyword term is a weight of 0.2 by default divided by `rrf_k + rank`. A row found by both channels gets both terms. An opt-in flag replaces the keyword rank term with a normalized BM25 magnitude. The fused score is then multiplied by:

- the memory's confidence (0.5 when unset);
- a recency decay of exp(-rate x age in days), with rate 0.001, 0.01 or 0.05 by stability tier of the memory type;
- a quality factor (1.0 high, 0.7 low, 0.9 otherwise);
- a confirmation boost (2.5 by default for confirmed memories, 1.2 for learned ones);
- a recap penalty of 0.3.

**Graph boost.** The graph step looks up up to five entities near the query. If none match, results are unchanged. The memory stream is on by default (`WENLAN_GRAPH_MEMORY_STREAM` turns it off). Each memory linked to an anchor entity that passes the type and hub-degree filter gets one more rank term of 1/(60+rank) when it is already in the results. Memories that were not retrieved are added only when the surface-new option is on, within a small budget. See Knowledge graph.

**Dedup, cap, normalize.** Knowledge-graph observation rows are removed from the output, a per-document cap is applied, and the list is cut to `limit`. `raw_score` keeps the unscaled score for callers that want their own relevance gate. `score` is divided by the best score the formula could give and capped at 1.0, so a weak top hit does not read as a perfect one.

**Cross-encoder.** The reranker takes the first 512 characters of each candidate and scores them on a blocking thread. If it errors or returns nothing, the original order stays. Otherwise its score replaces the fusion score, unless an opt-in blend flag mixes the two. `WENLAN_RERANKER_MODE` chooses the models: `off` (the default) uses no cross-encoder, `lite` uses the small turbo model on the quick, context and explicit `rerank` paths, and `full` uses turbo on the quick and context paths and the heavier `BGERerankerBase` on the `rerank` path. A non-empty environment value wins over the config setting.

## Pages

Page rows are not mixed into the memory ranking. After the memory search, `/api/memory/search` and `/api/search` look up pages themselves, skipping the "recent context" query. They read the caller's trust from the `x-agent-name` header, and an unknown caller counts as "unknown". Page search runs under the same Space scope and reads 9 candidates. It fuses a page vector search with a page keyword search (weight 0.2) into a 0 to 1 relevance. A page scoring under the floor of 0.3 is left out. A page that matched every query word by keyword is lifted to that floor, not dropped. The memory rows have no such floor.

The shared visibility gate then applies. Only callers with full or review trust see any page. Entity shadow pages are fenced out, each page's sources set its tier, and what is left is ranked and capped at 3. The result is the reply's `supplemental_pages`, which `recall` prints as "Compiled pages". How those pages are produced is in [How a page is written](how-a-page-is-written.md).

## Code

- [`crates/wenlan-core/src/db.rs` lines 2814-2822](../../crates/wenlan-core/src/db.rs#L2814-L2822)
- [`crates/wenlan-core/src/db.rs` lines 5304-5310](../../crates/wenlan-core/src/db.rs#L5304-L5310)
- [`crates/wenlan-core/src/db.rs` lines 28316-28348](../../crates/wenlan-core/src/db.rs#L28316-L28348)
- [`crates/wenlan-core/src/db.rs` lines 28422-28437](../../crates/wenlan-core/src/db.rs#L28422-L28437)
- [`crates/wenlan-core/src/db.rs` lines 28508-28532](../../crates/wenlan-core/src/db.rs#L28508-L28532)
- [`crates/wenlan-core/src/db.rs` lines 28573-28617](../../crates/wenlan-core/src/db.rs#L28573-L28617)
- [`crates/wenlan-core/src/db.rs` lines 28683-28724](../../crates/wenlan-core/src/db.rs#L28683-L28724)
- [`crates/wenlan-core/src/db.rs` lines 28977-29011](../../crates/wenlan-core/src/db.rs#L28977-L29011)
- [`crates/wenlan-core/src/db.rs` lines 29917-29974](../../crates/wenlan-core/src/db.rs#L29917-L29974)
- [`crates/wenlan-core/src/db.rs` lines 30446-30465](../../crates/wenlan-core/src/db.rs#L30446-L30465)
- [`crates/wenlan-core/src/db.rs` lines 30582-30602](../../crates/wenlan-core/src/db.rs#L30582-L30602)
- [`crates/wenlan-core/src/db.rs` lines 30640-30656](../../crates/wenlan-core/src/db.rs#L30640-L30656)
- [`crates/wenlan-core/src/db.rs` lines 44637-44680](../../crates/wenlan-core/src/db.rs#L44637-L44680)
- [`crates/wenlan-core/src/pages.rs` lines 162-210](../../crates/wenlan-core/src/pages.rs#L162-L210)
- [`crates/wenlan-core/src/read_scope.rs` lines 5-56](../../crates/wenlan-core/src/read_scope.rs#L5-L56)
- [`crates/wenlan-core/src/reranker.rs` lines 219-283](../../crates/wenlan-core/src/reranker.rs#L219-L283)
- [`crates/wenlan-mcp/src/tools.rs` lines 2351-2386](../../crates/wenlan-mcp/src/tools.rs#L2351-L2386)
- [`crates/wenlan-server/src/memory_routes.rs` lines 1279-1314](../../crates/wenlan-server/src/memory_routes.rs#L1279-L1314)
- [`crates/wenlan-server/src/memory_routes.rs` lines 1378-1424](../../crates/wenlan-server/src/memory_routes.rs#L1378-L1424)
- [`crates/wenlan-server/src/routes.rs` lines 242-268](../../crates/wenlan-server/src/routes.rs#L242-L268)

