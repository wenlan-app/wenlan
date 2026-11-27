# Refresh pages from changed evidence

Use this when sources, memories, or cited code made pages stale, or when new
material was attached to a page. Use a current `steep(action="list", space=...)`
worklist for the operation's scope. If the caller has none, or captures,
replacements or attachments since that list could have added stale pages,
list again first. For explicit reshaping, retain its requested accessible
scope and relevance/exclusion filter rather than silently reverting to a
current-project default or processing unrelated waiting pages. Process
at most ten pages per run, newest first. Skip leased pages and report pages
marked `needs_you` with their reason.

For each page, call `steep(action="claim", page_id=...)`. A refusal means
another run took it or its state changed; skip it. The claim provides current
sources, a token, and a plan. Never use `write_page` on a claimed page; only
the token-based `steep` actions may commit or release it.

- `plan="needs_you"`: make no change; report the claim's note.
- `plan="judge"`: give each code check to a smaller helper, using its
  `judge_instructions` and prompt verbatim. Submit the replies unchanged with
  `steep(action="judge", token=..., answers=...)`. If no smaller helper is
  available, release the claim. Never judge code impact with the main model.
  If the result remains stale, claim the page again for refresh.
- `stale_reason="source_updated"`: compare the page with its numbered
  surviving sources. A smaller helper may judge whether every sentence still
  matches; if none is available, rewrite. If the page still matches, call
  `still_accurate`; otherwise rewrite.
- `stale_reason="source_removed"`: rewrite to remove claims supported only
  by removed sources.
- `stale_reason="code_changed"`: inspect the current code and the claim's
  change details. If the page remains accurate, call `still_accurate`; if not,
  update only what conflicts and stop relying on deleted files. Code is not a
  numbered memory source, so don't attach `[n]` citations to code-only claims.
- Newly attached sources: integrate them with the page's existing content
  and citations.

For a rewrite, use the main model and only the claim's numbered sources. Keep
the title, language and headings. Cite every factual sentence as `[n]`. Keep
the page's own authored lines verbatim when `plan="review_card"` identifies
them. Preserve a Mermaid diagram unless its content changed; fix invalid
diagrams reported in `warnings`. A page that relies on other pages is marked
`needs_you`; follow the claim's note, leave it unchanged, and report that the
user should use Wenlan's **Update from sources** action.

Commit with `steep(action="commit", token=..., content=...)`. A citation
refusal may be fixed and retried once with the same token; otherwise release
it. On a version conflict, reclaim once and redo the judgment. A `review_card`
result means the saved page did not change; report it as pending. Never claim
a change happened until readback confirms it. Do not bypass a review lock or
the background job's claim.

Report updated page titles, pending review cards, pages left waiting, harmless
code changes checked, and each `needs_you` reason. The run is bounded to ten
pages; say another steep pass continues any remaining work.
