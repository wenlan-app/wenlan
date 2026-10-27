# First-use app verification

Run `pnpm exec vite --config vite.first-use-app.config.ts` and open
`http://127.0.0.1:1432/preview/first-use-app.html`.

This harness renders the production `Main`, Home, onboarding, import, settings,
and page components. It replaces Tauri IPC with the existing isolated test
runtime. It never falls back to the live daemon or reads the user's library.
It does not exercise the native setup gate or prove backend import behavior.

URL parameters:

- `locale=en`, `locale=zh-Hans`, or `locale=zh-Hant` (default).
- `theme=light` (default) or `theme=dark`.
- `state=empty` (default), `ready`, `processing`, `failed`, or `error`.
- `organize=deeplink|kimi|multi|writing|done|none|automatic|chats|folder` sets
  the import finish's "Turn them into pages" states (see
  `mocks/first-use-organize.ts`); `view=import` opens on the Import page and
  `view=sources` on the Sources library, `view=settings-sources` on Settings →
  Sources (`organize=folder`: a vault just added).

Use Home's quick tour entry, then choose the example or your knowledge. The
knowledge screen uses the actual app queries against the selected test state;
the built-in example stays local to the onboarding component. The `ready`
library is the existing deterministic navigation fixture. Clipboard actions use
the browser clipboard and report its actual result.

This harness verifies the integrated app rather than maintaining a second
onboarding UI.
Screenshots belong outside the repository.

## Live first-use preview (isolated scratch daemon)

`vite.first-use-live.config.ts` serves the same `preview/first-use-app.html`
and production `Main` on `http://127.0.0.1:1433`, with Tauri IPC mapped by
`preview/mocks/first-use-live-core.ts` to a scratch daemon. The fixture-only
`:1432` harness above is untouched. Start (root owns daemon launch):

```bash
WENLAN_PREVIEW_DAEMON=http://127.0.0.1:17878 \
WENLAN_PREVIEW_DATA_DIR=/tmp/wenlan-preview-scratch/data \
WENLAN_PREVIEW_KNOWLEDGE_PATH=/tmp/wenlan-preview-scratch/pages \
pnpm exec vite --config vite.first-use-live.config.ts
```

Fail-closed gates, no silent fixture fallback:

- Startup: all three variables required. The daemon URL must be exactly
  `http://127.0.0.1:17878` (absent, non-loopback, and prod `:7878` refused);
  both scratch paths must be absolute. The server never starts half-guarded.
- HTTP layer: `GET`/`HEAD` on `/api/*` plus five read `POST`s
  (`/api/search`, `/api/pages/search`, `/api/memory/list`,
  `/api/memory/entities/list`, `/api/memory/entities/search`) and two
  mutation `POST`s (`/api/import/memories` and
  `/api/onboarding/milestones/first-concept/acknowledge`); everything else is 403.
- Mutation layer (twice): the vite middleware re-verifies health +
  `GET /api/knowledge/path` against the scratch path BEFORE proxying the
  mutation (409 on mismatch); the browser re-verifies before sending it.
- Invoke layer: narrow read allowlist for Main/Home (incl. pending-review
  queue reads and `get_resolved_routing`)/first-use/ImportView/PageDetail
  plus the real `import_memories_cmd`. Setup probes, model download, client
  wiring, authored page writes, source registration, other milestone/review
  writes, and unknown commands all throw.

Known limitations: the daemon exposes no data-dir signal over HTTP
(`HealthResponse` is `{status, db_initialized, version}`), so data-dir
isolation rests on startup declaration plus launching the daemon against
it. `get_api_key` is `null` (no browser keychain),
`list_registered_sources` is `[]` (scratch starts sourceless; registration
is rejected), and `page_review_supported` is honestly
`"platform_unsupported"` (a browser cannot mint review capabilities).

The real first-page announcement can also be dismissed: only the `first-concept`
milestone acknowledgement is allowed, with the same scratch checks as import.
Other milestone writes and onboarding reset remain blocked.

### Verify automatic first-use processing

A successful non-empty import wakes the existing scheduler and starts a bounded
priority window (ten minutes, at most 64 enrichment slices, then Detect and
Emergence). This window does not require keyboard/mouse inactivity. Model consent,
provider availability, memory reserve, thermal protection, and shutdown still
apply. A previously selected cached local model can load for this explicit work
without waiting for CPU quiet samples; no model is downloaded automatically.

For automatic verification, use the real ImportView and inspect
`GET /api/import/batches/{batch_id}/status` plus `/api/ambient/status`. Do not call
`sweep` or `/api/steep` in that run: those are separate manual operator controls
and cannot prove the automatic path. Save the initial batch ID, inspect phase
failures, and wait for `complete`; disappearing from `/active` alone is not proof.
Memories can be searchable without a related page when there is not enough
suitable context. The UI reports that distinction instead of completing a
zero-page distillation step.

New libraries never generate a reserved Overview. Topic pages emerge from the
user's actual sources. Untouched Overview placeholders from older versions are
archived on database startup; useful existing, authored, or edited pages remain
intact. Rejected refreshes retain the old body and its complete citation map.
