# Turn new material into reusable knowledge

Use this when imported or saved material is not yet represented by a page, or
when new findings should be attached to an existing page. The calling steep
workflow supplies the resolved Space and session. Its initial `list` result
already established `new_total > 0`; do not repeat that call.

Work new material before refreshing pages, so attached sources can be handled
with their page. For an explicit goal, retain its relevance filter and
exclusions: leave unrelated groups alone even in the same Space.

1. Call `steep(action="new", limit=110, space=...)` once. Use the returned
   IDs, titles, snippets and existing `page_titles` to group material by a
   real topic. Do not fetch another page of results. Leave items out when
   there is not enough support for a coherent page; one long source can stand
   alone. A group may use an existing page's title when it belongs there.
2. Claim at most eight useful groups with `claim_new`, each using its exact
   source IDs and topic title. Make all claims together. Skip a refused claim.
   Claims reserve their source items; only use tokens returned by your own
   calls.
3. When a claim requests enrichment tasks, use a smaller helper if available.
   Give it only the token and task instructions; it fetches its own work and
   submits the answers through `steep(action="organize_tasks")` and
   `steep(action="organize")`. These are internal enrichment actions on the
   existing `steep` tool, not separate user workflows. Do not pass source
   text or helper answers through the main agent. If no smaller helper can run, process those tasks
   yourself for this run and report the fallback.
4. For each completed claim, call `steep(action="links", token=...)` just
   before deciding whether to write. If `existing_page` is set, call
   `steep(action="attach", token=...)` only if that page is within the goal's
   allowed scope (otherwise release the claim); the added sources make it
   eligible for refresh. Otherwise write one page with the main model using
   only the claimed sources. Cite every factual sentence with its numbered
   source (`[1]`), and add only returned, relevant `link_targets` as
   `[[Title]]`. Keep the page in the claim's Space. Call `steep(action="write",
   token=..., title=..., content=...)`. On citation refusal, fix the cited
   sentences and retry once; if still refused or sources do not cohere,
   release your claim.

Never let a helper write a page. Do not write a page for a claim's sources
through `write_page`; the claim's `write` or `attach` action is the guarded
write path. Respect existing page identity and don't create a duplicate when
the claim identifies a matching page.

When sources were attached, call `steep(action="list", space=...)` again:
those pages were not waiting in the initial worklist. Load
[refresh-pages.md](refresh-pages.md) even when the initial `total` was zero,
and process relevant pages in the fresh worklist, retaining the goal's scope
and shared run budget when present. Report new pages by title and attached source
counts. If material remains, say how much is waiting and that another steep
pass continues it. The current run is bounded to eight new groups.
