# Citations and evidence-driven updates

[English](citation-checks.md) · [繁體中文](citation-checks.zh-Hant.md) · [Steep guide](steep.md)

Wenlan tracks page dependencies and checks numbered citations in generated drafts. These are separate mechanisms: dependencies identify pages affected by changed evidence; citation checks assess how cited text overlaps with the support supplied for that draft.

## What is recorded

A page retains its supporting source and memory references. Each processed citation occurrence records its marker, source kind and locator, score, `verified` or `unverified` status, and `sentence` or `paragraph` scope. A marker reused in two places can have different results.

A tracked source change marks dependent pages stale, advances their source revision, and clears old citation results. Refresh and annotation writes use version/revision checks to avoid applying results computed for an outdated page or evidence state. This page-level revision tracking is not a separately versioned evidence ledger for every factual claim.

## What the citation check does

1. Normalize numbered markers and remove markers outside the supplied source list.
2. Compare each cited sentence with the combined text of the sources it cites. The support score uses the greater of the two directions of content-token overlap; the current threshold is **0.5**.
3. If the sentence does not pass, retry its enclosing paragraph and record that broader scope. For multiple sources, the shared status comes from their combined text; each citation also records its individual-source score.
4. In the shared synthesis path used for page refresh, reject a draft with no verified citation occurrences or more unverified than verified occurrences. The saved page is not replaced by that rejected draft.

A passing refresh still follows the [page ownership and review rules](steep.md#automatic-update-or-review). The gate is not a requirement on every manual or explicit agent edit.

## What it does not prove

- `verified` means the text passed this overlap check. It does not prove that the source is true, the conclusion follows logically, or multiple sources agree.
- Statements without citation markers are outside this check. Paragraph-level support is broader than support for the individual sentence.
- Sentence splitting uses `.`, `!`, or `?` followed by whitespace; token filtering uses English stopwords. This is not a language-aware semantic verifier, and its scope and scores need not behave equally across languages.
- The refresh synthesis path currently supplies up to **800 characters per source**. The check uses those supplied snippets, not the complete original document or an independent web search.
- External changes must first be read into Wenlan. A recent page timestamp alone does not establish that its evidence is current.

Use citations to inspect the original support and history to inspect changes. Treat automated check status as an inspection aid, not a guarantee of correctness.

Implementation: [citation records and scoring](../crates/wenlan-core/src/citations.rs), [synthesis gate](../crates/wenlan-core/src/synthesis/distill.rs), [dependency invalidation](../crates/wenlan-core/src/db.rs), [canonical page updates](../crates/wenlan-core/src/post_write/page_update.rs).
