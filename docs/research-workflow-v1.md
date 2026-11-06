# Research workflow integration

This implementation connects a local Zotero library to Wenlan's existing
Sources, Spaces, source reader, external-tool handoff, page editor and review
queue. It does not introduce a research workspace, chat sidebar or Query screen.

## User path

1. In Settings → Sources, or a Space → Sources, choose a local Zotero collection
   and a destination Space. Import the collection or choose individual articles.
   Import is manual. Zotero must enable its local API.
2. Read the imported papers in the existing source list. A Zotero citation opens
   PDF in the page's existing right inspector. Info and Mind map retain their
   original icons in the upper-right toolbar; PDF joins them only after opening
   a paper. There is no second row of inspector tabs. Clicking the active tool
   collapses the pane; the PDF icon remains available for that page. Each Wiki
   retains its attachment, page, zoom and search text within the app session.
   Two document groups share one right inspector following the focused Wiki;
   returning to a Wiki restores its own context. Desktop panes reserve document
   space; narrow windows show the same pane over the document without unmounting
   the editor. PDF controls stay compact and the document fills the pane height.
   Reuse the inspector's resize and expand controls, or the full-source view.
   Search, select text, navigate pages and zoom using bundled PDF.js. Imported
   Zotero annotations use actual PDF coordinates. An explicitly checked citation
   can carry an immutable PDF digest, page and quote; an ambiguous quote opens
   its page without inventing a highlight. Saved PDFs remain readable offline.
   External opening and an explicit missing-cache retry remain optional.
3. Select sources to prepare a summary and classification suggestions in an
   external AI tool. Default instructions are included; custom instructions are
   optional. A durable run records the exact sources, the Space description as
   research-purpose context, rule revision, reading coverage and destination
   page. Purpose and organizing rules remain separate: editing either applies
   to a new run; resuming retains the original snapshot. Legacy runs without a
   purpose snapshot do not borrow today's description. Hypotheses in the
   description remain questions to test, not evidence. The dialog can continue
   an existing research page and show source additions/removals relative to its previous run. Cancelled
   or partial runs retain progress and require an explicit continuation action.
4. The local MCP `read_source` tool reads each selected document in bounded
   Unicode-character pages. Every response carries a whole-text digest; callers
   pass `organize_run_id` to record the text actually returned for that run.
   Skipped slices or a model's completion assertion cannot complete coverage.
   Changed source generations require a new run. Completion means all
   **indexed text**, not proof that every PDF figure/table was parsed or understood.
5. The AI writes a knowledge page using the existing write permissions and review
   policy. Structured classification proposals use the same Review queue with
   reasons, current tags and editable additions/removals. Human tag decisions
   and dismissals survive reruns. Manual synonym merging applies atomically to
   the selected sources; unrelated tags are preserved.
6. Existing review defaults to accepting the whole proposal. Optional **Adjust**
   edits the proposed body before accepting. The proposal body and target page
   version are checked in the write transaction; a conflict preserves the draft.
   Source-backed changes to protected human text, and writes to review-locked
   pages, use this whole-page review. Permitted AI-only changes can update directly.
7. Continue writing in the existing editor. The export dialog offers retained
   versions in Markdown, HTML or BibTeX. Each export bundle fixes the body,
   citation map, source presentation and bibliography together. Opening the
   dialog prepares the current saved version; older retained versions can be
   selected. A pending draft is separately labeled as not adopted. Old history
   without retained references is unavailable for a complete referenced export.
   The selected bundle is reauthorized after the native save dialog. It never
   borrows today's references for an older body. Partial bibliography is visible;
   BibTeX requires all selected bibliography entries.


## Data and failure contracts

- Production Zotero reads use the fixed loopback API on port 23119. Registration
  records its server identity; a missing or different identity fails closed.
- Imported records retain `source=file`, `source_agent=zotero`. Their identity
  includes the server, library, item and Space ID, rather than title or DOI.
  Repeated collections in the same Space do not duplicate a paper.
- Sync does not overwrite manual tags. Transient attachment failures preserve
  the prior successful content/cache. Each unsuccessful item is reported; a
  corrupt metadata sidecar does not abort unrelated items.
- A complete collection listing can identify previously imported papers that
  left the selection. Partial or failed listings cannot establish absence.
  Affected pages require review without treating a selection change as a
  retraction or deleting the retained paper. The review requirement survives
  automatic refreshes and is resolved only by adoption against the same evidence
  generation. An in-flight machine write cannot bypass this requirement.
- Disconnecting Zotero stops synchronization and keeps imported documents,
  PDFs, tags, human text and old exports. If disconnecting is interrupted, the
  source list offers **Finish disconnect**, using a fresh impact preview.
  Legacy registrations without import provenance show an unknown impact scope
  rather than inventing a complete affected-paper count.
- Bibliographic changes participate in the source digest. Prior source text and
  bibliography snapshots are retained. PDFs are cached by content digest.
- Zotero document and PDF reads require a Space and acknowledge that scope.
  A conflicting strict Space header is rejected. Imported metadata never grants
  an arbitrary filesystem read or arbitrary URL fetch.
- Citation checks read the exact declared sources in the target Space, including
  later indexed text rather than only the first chunk. Each claim is scored
  against a bounded contiguous passage; a lexical match is not a semantic or
  scientific validation. Source fingerprints are checked in the same transaction
  as create, update, staging, attachment and acceptance. Changed or unreadable
  sources refuse the write and ask the caller to read them again. Accept rechecks
  the adopted text; human adjustments never inherit a stale verified status.
- Annotation links use actual attachment and annotation keys plus known page
  indices. The reader accepts only links to that document's attachment inventory.
  It does not fabricate highlight coordinates.
- Source selection and unfinished instructions survive navigation within the
  session. Wiki-specific PDF context is bounded in-memory session state; the
  standalone source reader retains its existing local reading position. Imported documents, pages,
  pending revisions and source cache survive daemon restarts.
- Run progress, classification decisions and page-write binding are transactional.
  Coverage cannot claim success for missing inputs. A review proposal keeps the
  run awaiting review until actual adoption; dismissal remains distinguishable
  from completion. Source, rule, manual-decision and lifecycle generations are
  checked rather than trusting a model's completion report.
- Sourced disagreements use the existing Review queue. Evidence quotes, PDF
  targets and study conditions remain attached to human decisions. Changed or
  missing evidence cannot be confirmed as fresh; it can still be deferred or
  closed with a recorded reason. Unchanged closed proposals do not reappear
  merely because the AI reruns or rephrases its explanation.
- Explicit source exclusion, retraction and removal retain the document and its
  history. Affected references remain navigable as historical evidence and need
  rechecking. The source's withdrawal does not disprove every statement that
  cited it, and other independent evidence is retained. This is not automatic
  monitoring of worldwide retraction notices.
- No model/API key is needed to import, read, edit or classify manually. Automatic
  summaries require an external AI tool configured by the person.
- Manual edits retain identifiable citation references after known markers are
  deleted or reordered. Exact PDF regions survive only for unambiguous unchanged
  paragraphs; ambiguous new associations fail closed. Retained references become
  unevaluated (source not checked yet), with old source stamps kept as history. This neutral state
  neither inherits support for a rewritten claim nor claims the source failed
  a check. A subsequent source check can verify or reject the claim.
  An agent refresh that leaves the page unchanged reports that outcome and emits
  no "updated" receipt.

## Acceptance boundaries

English 20-paper native evidence, current renderer fixtures, backend contracts
and final integrated acceptance are separate receipts. Passing source coverage
or lexical matching does not establish scientific correctness or completion of
all fourteen researcher scenarios. The canonical researcher spec owns that
acceptance checklist.

- Cross-language semantic checking remains deferred; the current corpus and
  synthesis acceptance use English.
- Only explicit PDF targets carry page/quote guarantees. A generic citation can
  still open the document without an exact region. Imported annotation overlay
  fixtures do not by themselves prove real Zotero acquisition.
- Old history cannot retroactively recover references or bibliography that were
  never retained. Current capture is labeled with its actual capture time.
- Unknown or malformed citation markers, ambiguous new associations, or a legacy
  page with an incomplete map can require rebuilding the map. This is not a
  general citation-marker editor.
- No Zotero writes, annotation editing, automatic polling or cloud OAuth are
  introduced. Selection-loss checks occur during explicit synchronization;
  they are distinct from manually excluding or retracting a source.
- Native packaged WebView, save dialogs, restart and offline behavior must be
  exercised separately from browser mocks. Windows GUI is not established by
  Windows CI. Final integrated native verification is still required for the
  completion changes described here.


## Verification entry points

- Core: `research_`, `citation_provenance`, `page_export`, `zotero`,
  `citation_snapshot`, `edited_acceptance`, `patch_document_tag`
  focused tests, including late-body evidence and transaction race controls.
- Server: `zotero`, edited-acceptance HTTP contract, route catalog checks.
- MCP: `read_source` tests plus the full MCP library/tool-surface suite.
- Frontend: source selection, Zotero import, PDF links, review adjustment and
  immutable-version export tests; TypeScript and localization checks.
- Renderer: `E2E_PORT=14336 pnpm exec playwright test e2e/pdf-reader.spec.ts`.
  This uses a repository public-paper fixture and bundled PDF.js without remote
  asset requests; it checks search, navigation, zoom and reading-position restore.
- Live browser preview: set `WENLAN_PREVIEW_SOURCE_WRITES=1` and explicitly set
  `WENLAN_PREVIEW_DAEMON` to an isolated loopback daemon, never port 7878. Verify
  `/api/knowledge/path` before any test writes. The browser export shim downloads
  actual generated bytes but does not prove a native Tauri save dialog.

Screenshots and machine-specific verification receipts belong with the PR,
not in this repository.
