# Technical foundations

This page describes Wenlan's current production retrieval and graph paths. It
separates core behavior from optional channels so a method being available is
not mistaken for it always running. The source tree and `Cargo.lock` remain the
version-level sources of truth.

## Retrieval pipeline

```text
query
  |-- SQLite FTS5 -------------------- lexical ranking
  `-- BGE dense embedding
        `-- libSQL cosine DiskANN ---- semantic ranking
                    |
          weighted RRF (k = 60)
                    |
          base Memory candidate pool
             |-- graph-linked Memory boost (quick path, when links exist)
             |-- optional Page / episode / fact channels (deep path)
             `-- optional cross-encoder reranking
```

The base path is hybrid retrieval, not a single vector lookup:

- **Lexical retrieval (core):** [SQLite FTS5](https://www.sqlite.org/fts5.html)
  indexes Memory content and titles to find literal words, identifiers, and
  phrases.
- **Dense retrieval (core, local, no API key):**
  [FastEmbed](https://github.com/Anush008/fastembed-rs) with
  [`Qdrant/bge-base-en-v1.5-onnx-Q`](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q)
  encodes queries and Memories independently as 768-dimensional English dense
  vectors.
- **Vector index (core):**
  [libSQL vector search](https://docs.turso.tech/features/ai-and-embeddings)
  stores `F32_BLOB(768)` vectors and uses cosine DiskANN to retrieve approximate
  nearest neighbors without scanning the full global collection.
- **Rank fusion (core):**
  [Reciprocal Rank Fusion](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf)
  with `k = 60` combines lexical and vector rankings without requiring their raw
  scores to share a scale.
- **Graph context (default on, data-dependent):** Entity embeddings and the
  `memory_entities` junction boost linked candidate Memories while preserving
  the active Memory read scope.
- **Cross-encoder reranking (off by default):**
  [`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en)
  or
  [`BAAI/bge-reranker-base`](https://huggingface.co/BAAI/bge-reranker-base)
  reads each query-candidate pair through FastEmbed and reorders a smaller
  candidate pool.

Wenlan's fusion is RRF-derived rather than textbook equal-weight RRF. With the
default magnitude-fusion flag off, the vector contribution is
`cosine_similarity / (60 + rank)` and the lexical contribution is
`0.2 / (60 + rank)`, before any query-intent channel multipliers. The optional
magnitude-fusion path replaces lexical rank with normalized BM25 magnitude but
keeps the same `0.2 / 60` maximum contribution.

For global vector search, libSQL's `vector_top_k` uses the DiskANN index. A
selected Space applies its scope filter before cosine ordering instead of
querying the global index, preserving the requested read boundary.

## Connected knowledge model

The README visual is a product-level model, not a claim that every object lives
in one homogeneous graph table. Wenlan connects four explicit structures:

- **Page links:** `[[wikilinks]]` resolve into `page_links`, including orphan
  labels that can resolve when the target Page appears later.
- **Page evidence:** `page_evidence` records whether a Page is supported by a
  Memory, external URL, external file, or authored material.
- **Memory-Entity links:** `memory_entities` connects each atomic Memory to
  every extracted Entity while the Memory retains its original provenance.
- **Entity relations:** `relations` stores directed, typed edges between
  Entities, with optional confidence, explanation, and source-Memory
  provenance.

Together these structures keep Pages readable and source-backed while the
entity graph remains available for connection, grouping, and retrieval. They
have different ownership and update rules, so Wenlan does not collapse them
into one undifferentiated graph store.

## Graph data and entity resolution

With an enrichment model configured, Wenlan extracts and stores:

- **Entities:** named people, projects, concepts, places, and other typed nodes.
- **Typed relations:** directed edges such as one entity using or belonging to
  another. A relation can store confidence, an extraction explanation, and
  source-Memory provenance.
- **Observations:** claims attached to an entity.
- **Memory links:** a many-to-many `memory_entities` table connecting each
  Memory to every extracted entity, while the Memory retains its original
  source.

Relation types are normalized against a seeded vocabulary. An unknown
snake-case type is stored as `related_to` and queued as a reviewable
`vocab_promote` proposal, so the vocabulary can grow without silently
fragmenting the graph.

Entity resolution follows an explicit cascade:

1. Reuse a registered alias.
2. Reuse an exact name match.
3. Optionally check same-type near-duplicates with MinHash/LSH and exact
   Jaccard similarity (`>= 0.9`).
4. Reuse the nearest BGE entity vector when cosine similarity is above `0.9`.
5. Create a new entity only when the earlier steps do not resolve the mention.

The refinement cycle also runs label-propagation community detection over the
entity-relation graph. For this grouping step, relations are treated as
undirected and each entity pair is weighted by its number of distinct relation
rows; relation confidence is stored separately and is not the community edge
weight. The resulting `community_id` can group linked Memories for the optional
global-context summary path.

Automatic post-ingest extraction assigns each newly created entity the source
Memory's Space, or Uncategorized when the source has no Space. The selected-Space
graph stream still depends on eligible entity links being present; this does not
backfill legacy entity rows or establish retrieval parity. Returned Memories
remain filtered by the active read scope.

The extraction and commit path is in
[`kg/entity_extraction.rs`](../crates/wenlan-core/src/kg/entity_extraction.rs);
the resolution cascade is in
[`post_write.rs`](../crates/wenlan-core/src/post_write.rs).

## Graph-assisted retrieval

The default graph-memory stream is a bounded ranking signal, not an
unrestricted graph expansion:

1. Embed the query and retrieve the top entity candidates.
2. Remove person-like anchors and high-degree hubs.
3. Find linked Memories through `memory_entities` in the active
   [`ReadScope`](../crates/wenlan-core/src/read_scope.rs).
4. Add one graph RRF term, `1 / (60 + graph_rank)`, per linked Memory already
   present in the candidate pool.

This stream is default-on for the quick path, but it is data-dependent: an
empty or unenriched graph contributes nothing, and a selected Space needs
Space-scoped entity anchors. Surfacing graph-only Memories is a separate opt-in
behavior; the default stream is boost-only. A live deep cross-encoder pass
suppresses this stream, while the standalone light reranker for quick and
context requests runs after base retrieval and can reorder a graph-boosted
pool.

## Optional channels and defaults

| Capability | Default | Behavior when enabled |
|---|---|---|
| Graph-memory stream | On | Boosts linked Memories when eligible entity links exist. |
| Page channel | Off | Searches maintained Pages separately and appends them as supplementary context. |
| Episode channel | Off | Adds verbatim episodic rows as another RRF stream. |
| Fact channel | Off | Retrieves per-fact child vectors, then rehydrates their parent Memories. |
| Cross-encoder reranking | Off | `lite` uses Jina Turbo on quick, context, and explicit deep rerank paths; `full` uses Jina Turbo on quick/context and BGE Reranker Base for explicit deep reranking. |
| On-device language model | User-selected | Enables local extraction, enrichment, and Page synthesis after the selected model is downloaded. |

Page, episode, and fact-channel failures are logged and fall back to the
remaining retrieval signals. If a cross-encoder fails or returns no scores,
Wenlan preserves the pre-rerank ordering.

## Typed Memory schema

Wenlan's llm-wiki schema foundation is concrete in `MemorySchema`: each Memory
type has required and optional structured fields plus a retrieval-cue template.
`identity`, `preference`, and `decision` have specialized shapes; `fact`,
`lesson`, `gotcha`, and unknown types currently use the fact shape. Validation
reports missing required fields, and enrichment can turn populated fields into
deterministic retrieval cues.

This is a typed Memory schema, not a claim that every Page has a user-editable
schema. Page structure is governed separately by the synthesis prompts,
canonical Page write path, provenance records, citation processing, and review
rules. The [citation-check reference](citation-checks.md) ([繁體中文](citation-checks.zh-Hant.md))
details sentence/paragraph support checks, source-change invalidation, and
what a `verified` status does and does not establish.

## Model roles

| Role | Current choices | Notes |
|---|---|---|
| Dense embedding | `Qdrant/bge-base-en-v1.5-onnx-Q` | Quantized ONNX, 768 dimensions, English; runs locally through FastEmbed. |
| Light cross-encoder | `jinaai/jina-reranker-v1-turbo-en` | Optional English reranker for latency-sensitive paths. |
| Deep cross-encoder | `BAAI/bge-reranker-base` | Optional reranker for explicit deep search in `full` mode. |
| On-device language model | [`unsloth/Qwen3-4B-Instruct-2507-GGUF`](https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF), file `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` | Smaller user-selected option for enrichment and synthesis. |
| On-device language model | [`unsloth/Qwen3.5-9B-GGUF`](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF), file `Qwen3.5-9B-Q4_K_M.gguf` | Larger user-selected option for enrichment and synthesis. |

On-device language models run through Rust bindings for
[`llama.cpp`](https://github.com/ggml-org/llama.cpp). Daemon startup does not
download one implicitly: it loads a model only when the user has selected it
and the file is already cached. OpenAI-compatible local endpoints such as
Ollama or LM Studio and configured cloud providers remain alternatives.

## Current limits

- The dense embedding model is English. FTS5 still provides literal matching
  for other languages, but Wenlan does not claim multilingual dense-retrieval
  parity from this model.
- A graph can improve retrieval only after model-backed extraction, imported
  graph data, or explicit entity links have created the required substrate.
- Newly created automatic post-ingest entities inherit the source Memory's Space
  (or Uncategorized when absent). This does not backfill legacy rows or guarantee
  selected-Space graph retrieval; Memory results remain scope-filtered.
- Bounded k-hop BFS exists behind `WENLAN_ENABLE_GRAPH_KHOP`, but its current
  path feeds the legacy observation branch whose rows do not survive Memory
  output. It is not a live Memory-retrieval channel.
- Cross-encoders rerank only the candidates they receive; they cannot recover a
  Memory omitted from the candidate pool.
- Optional channels increase recall paths and runtime cost. Their default-off
  state is intentional and should remain visible in product claims.
- "Local" describes where inference and storage run. Model files may still
  need a one-time download before first use.

## Implementation entry points

- Hybrid retrieval, vector schema, RRF, graph stream, and optional channels:
  [`db.rs`](../crates/wenlan-core/src/db.rs)
- Page links, typed evidence, Memory-Entity links, and Entity relations:
  [`db.rs`](../crates/wenlan-core/src/db.rs) and
  [`synthesis/wikilinks.rs`](../crates/wenlan-core/src/synthesis/wikilinks.rs)
- Graph extraction and Memory-entity linking:
  [`kg/entity_extraction.rs`](../crates/wenlan-core/src/kg/entity_extraction.rs)
- Entity resolution:
  [`post_write.rs`](../crates/wenlan-core/src/post_write.rs)
- Typed Memory fields and retrieval-cue templates:
  [`schema.rs`](../crates/wenlan-core/src/schema.rs)
- Bounded graph-traversal scaffold (not a live Memory-retrieval channel):
  [`retrieval/traversal.rs`](../crates/wenlan-core/src/retrieval/traversal.rs)
- Cross-encoder modes and fallback contract:
  [`reranker.rs`](../crates/wenlan-core/src/reranker.rs)
- On-device model registry:
  [`on_device_models.rs`](../crates/wenlan-core/src/on_device_models.rs)
- Selected-and-cached startup gate:
  [`wenlan-server/main.rs`](../crates/wenlan-server/src/main.rs)
- Exact dependency versions:
  [`Cargo.lock`](../Cargo.lock)

## Primary references

- [SQLite FTS5](https://www.sqlite.org/fts5.html)
- [Reciprocal Rank Fusion, Cormack, Clarke, and Buettcher (SIGIR 2009)](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf)
- [FastEmbed for Rust](https://github.com/Anush008/fastembed-rs)
- [BGE Base EN v1.5 quantized ONNX model card](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q)
- [libSQL vector search](https://docs.turso.tech/features/ai-and-embeddings)
- [DiskANN in libSQL](https://turso.tech/blog/approximate-nearest-neighbor-search-with-diskann-in-libsql)
- [Jina Reranker v1 Turbo EN model card](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en)
- [BGE Reranker Base model card](https://huggingface.co/BAAI/bge-reranker-base)
