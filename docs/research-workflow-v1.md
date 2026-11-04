# Research workflow integration

This implementation connects a local Zotero library to Wenlan's existing
Sources, Spaces, source reader, external-tool handoff, page editor and review
queue. It does not introduce a research workspace, chat sidebar or Query screen.

## User path

1. In Settings → Sources, or a Space → Sources, choose a local Zotero collection
   and a destination Space. Import is manual. Zotero must enable its local API.
2. Read the imported papers in the existing source list. Open a PDF alongside
   the page, or use the existing full-source view. Search, select text, navigate
   pages and zoom using bundled PDF.js. Saved PDFs remain readable offline.
3. Select sources to prepare a summary and classification suggestions in an
   external AI tool. Default instructions are included; custom instructions are
   optional. The prompt names the exact sources and destination Space.
4. The local MCP `read_source` tool reads each selected document in bounded
   Unicode-character pages. Every response carries a whole-text digest; callers
   must restart a document if that digest changes between pages. Completion
   means all **indexed text**, not proof that every PDF figure/table was parsed.
5. The AI writes a knowledge page using the existing write permissions and review
   policy. Classification proposals remain text for human review. Manual tag
   additions/removals are atomic and the source list supports tag filtering.
6. Existing review defaults to accepting the whole proposal. Optional **Adjust**
   edits the proposed body before accepting. The proposal body and target page
   version are checked in the write transaction; a conflict preserves the draft.
   Source-backed changes to protected human text, and writes to review-locked
   pages, use this whole-page review. Permitted AI-only changes can update directly.
7. Continue writing in the existing editor. Export the current saved page as
   Markdown, HTML or BibTeX from its action menu. Export captures the displayed
   saved version and rechecks its inputs after the save dialog; changed inputs
   require retry. This is optimistic validation, not a server transaction.
   If some bibliography entries cannot be read, Markdown and HTML remain
   available with a warning; BibTeX waits for every entry.

## Data and failure contracts

- Production Zotero reads use the fixed loopback API on port 23119. Registration
  records its server identity; a missing or different identity fails closed.
- Imported records retain `source=file`, `source_agent=zotero`. Their identity
  includes the server, library, item and Space ID, rather than title or DOI.
  Repeated collections in the same Space do not duplicate a paper.
- Sync does not overwrite manual tags. Transient attachment failures preserve
  the prior successful content/cache. Each unsuccessful item is reported; a
  corrupt metadata sidecar does not abort unrelated items.
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
  session; PDF reading position persists locally. Imported documents, pages,
  pending revisions and source cache survive daemon restarts.
- No model/API key is needed to import, read, edit or classify manually. Automatic
  summaries require an external AI tool configured by the person.
- Manual edits retain existing, fully mapped citation targets when the numbered
  marker sequence is unchanged. Retained references become unevaluated (source
  not checked yet), with old source stamps kept as history. This neutral state
  neither inherits support for a rewritten claim nor claims the source failed
  a check. A subsequent source check can verify or reject the claim.
  An agent refresh that leaves the page unchanged reports that outcome and emits
  no "updated" receipt.

## Acceptance boundaries

The three-public-paper live walkthrough and the 20-plus-item synthetic sync
regression exercise different things. Neither is a 20-paper summary-quality
evaluation or proof that every researcher-spec scenario is complete.

The following remain explicit boundaries for subsequent researcher acceptance:

- AI classification is a proposal in the knowledge page, not a structured tag
  proposal queue with per-tag provenance and accept/reject controls.
- A page citation opens its source PDF. Exact page navigation is supported for
  imported annotation links with a page index; generic page citations do not yet
  carry a PDF page/region locator.
- Import selects a collection, not an arbitrary subset before import. The user
  selects papers for subsequent organization in the Space source list.
- Export selects the current saved version, not historical page versions.
- Adding, removing or reordering citation markers, or editing a legacy page with
  an incomplete/unstamped citation map, still clears the map pending a rebuild.
  This integration does not introduce a general citation-marker editor.
- No Zotero writes, annotation editing, automatic polling or cloud OAuth are
  introduced. Removing a connection retains already imported content.
- Research-specific disagreement decisions, retraction workflows and structured
  classification provenance are not claimed complete by this integration.
- Windows/macOS packaged WebView and native save-dialog checks are separate from
  the browser verification. Unit mocks do not establish native filesystem access.

## Verification entry points

- Core: `zotero`, `citation_snapshot`, `edited_acceptance`, `patch_document_tag`
  focused tests, including late-body evidence and transaction race controls.
- Server: `zotero`, edited-acceptance HTTP contract, route catalog checks.
- MCP: `read_source` tests plus the full MCP library/tool-surface suite.
- Frontend: source selection, Zotero import, PDF links, review adjustment and
  snapshot export tests; TypeScript and localization checks.
- Renderer: `E2E_PORT=14336 pnpm exec playwright test e2e/pdf-reader.spec.ts`.
  This uses a repository public-paper fixture and bundled PDF.js without remote
  asset requests; it checks search, navigation, zoom and reading-position restore.
- Live browser preview: set `WENLAN_PREVIEW_SOURCE_WRITES=1` and explicitly set
  `WENLAN_PREVIEW_DAEMON` to an isolated loopback daemon, never port 7878. Verify
  `/api/knowledge/path` before any test writes. The browser export shim downloads
  actual generated bytes but does not prove a native Tauri save dialog.

Screenshots and machine-specific verification receipts belong with the PR,
not in this repository.
