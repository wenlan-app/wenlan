# Page saves and Markdown recovery

The daemon owns the canonical Page row, version CAS, source links, history and
operation receipt. The manual edit endpoint also projects the latest row into
the configured Markdown directory. Its path comes from ServerState, so isolated
tests never fall back to a personal vault.

## Response contract

POST `/api/memory/{id}/update-page` returns `ok: true` after a successful DB write
or unchanged save. The additive `projection_status` is:

| Value | Meaning |
|---|---|
| `synced` | The latest loaded DB row was projected (or its exact rendered bytes already existed). |
| `pending` | DB saved; Markdown repair failed or external edits were preserved. `projection_error` gives the reason. |
| `hidden` | The existing truth gate does not permit automatic projection. |
| `not_configured` | This server state has no projection root. |

The old typed PageWriteResponse still decodes the response as saved. A frontend
must not treat `pending` as an unsaved DB draft or mint a new operation to retry
that committed text. Replaying the original operation repairs the latest DB row,
even if a later edit has already advanced it. An unchanged save repairs missing
files without another Page version. Neither repair changes the DB receipt.

## External edits and interruption

Each state entry records SHA-256 of the exact installed Markdown bytes. A stale
version stamp alone never authorizes replacing a file. Differing external text
is preserved unless it matches the last written bytes or has already been
adopted as canonical Page prose. Legacy entries without a digest establish one
when their prose matches DB; unknown differing prose stays intact.

Before replacement, the writer moves the old inode into
`.wenlan/projection-recovery/`, validates the inode actually moved, and installs
the new file with a create-only hard link. An external editor recreating the old
pathname cannot be overwritten. Editors still writing through an open old
handle keep a reachable recovery file. These recovery copies are retained; this
change does not automatically merge them or delete them. A reader can briefly
observe a missing pathname during installation, but never partial new prose.
Filesystems without hard-link support report `pending` and preserve the DB save.

State is saved by atomic replacement after the content installation. A retry
repairs a stale state entry, and a first projection installed before its state
save is reused by its origin id instead of creating a second filename.
Explicit receipted repair operations retain their existing captured-byte guards.

Desktop IPC currently maps this additive daemon reply to its existing Saved
result. Showing a low-interruption pending/conflict hint requires a subsequent
frontend change; daemon status alone is not proof of that UI.

## Archive and restore

POST `/api/pages/{id}/archive` archives the row, then moves the page's file out
of the pages folder into `archive/` without rewriting it, so unsynced edits
survive there and no file in the pages folder poses as an active page (the
watcher would skip its edits as stale, and a restore would refuse to replace
them). The response adds `projection_status`: `synced` once the file is moved,
`pending` with `projection_error` when it could not be; the next daemon start
retries the move for every archived page (the desktop app does not show
`pending` yet). Only files whose frontmatter names the page move, so a note
someone put at the page's old path stays put and is reported; and only files
stamped no newer than the archived version, so a restore that projected the
page in between keeps its file. The moved file is checked again after the
move and put back if it is not the page's. Archive and restore hold one
lifecycle lock across their database change and projection, and archive moves
the file only while the page is still archived at the version it produced, so
a restore whose projection failed keeps its file. State remembers the folder the page
was archived from. Restore projects the restored version as a fresh file in
that folder (or the creation folder if it is gone) and leaves the copy in
`archive/`, with any edit made to it, where it is.
