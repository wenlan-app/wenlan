# wenlan-server reference

Module map and RB-01 profiling flag reference. Read the active `AGENTS.md` first and
load this file only when the task touches the corresponding area.

Applies to agents working under `crates/wenlan-server/`. Read alongside root
`AGENTS.md`, which takes precedence on any topic not covered here.

## Key Modules (`crates/wenlan-server/src/`)

Route modules follow the file name: `*_routes.rs` owns the handlers for that surface and exposes one or more `TrackedRouter` registration helpers, which `router.rs` composes. Where a module has two or more helpers it is because their composition positions are separated and must stay that way. Read the directory for the current set.

The modules whose job is not evident from the name:

| Module | Purpose |
|---|---|
| `main.rs` | Binary entry — daemon startup plus internal maintenance commands, tracing init, port binding with existing-daemon fallback, `MemoryDB::new`, LLM provider init, background tasks, `axum::serve` |
| `state.rs` | `ServerState` with `db: Option<Arc<MemoryDB>>`, `shutdown`, `bound_port`, the LLM handles (`llm`, `api_llm`, `synthesis_llm`, `external_llm`), `reranker` + `reranker_status`, `prompts`, `tuning`, `quality_gate`, `write_signal`, `maintenance_coordinator`, `ingest_batcher`, `repair_root`, `presence_root`, `lint_config`. `SharedState = Arc<RwLock<ServerState>>` |
| `router.rs` | Axum composition root — assembles the module-owned registration helpers plus the remaining inline registrations, then applies the truth/security/lifecycle layers |
| `routes.rs` | General endpoints: health, status, search/context, diagnostics, recent activity, steep/distill |
| `memory_routes.rs` | Memory CRUD/search/enrichment, classification, statistics, scheduler-handoff, rerank, attribution, update, revision, contradiction tests |
| `ingest_batcher.rs` | Request-level coalescer for concurrent `/api/memory/store` — folds QualityGate in-line, async classify/extract, passes enrichment + hint through in the response |
| `scheduler.rs` | Background periodic tasks (distill cycles, distillation, the reconcile/backfill sweeps gated by the `WENLAN_ENABLE_*` flags) |

## Manual RB-01 profiling flags

These flags control ignored, target-Mac profiling tests; they are not daemon runtime settings and must not be set in normal service configuration.

| Flag | Contract |
|---|---|
| `WENLAN_RB01_BASELINE` | Set to `1` to opt into the five-minute daemon-off resource baseline test. |
| `WENLAN_RB01_THERMAL_HELPER` | Optional path to the frozen helper executable that prints the macOS `ProcessInfo.thermalState` raw value; the test falls back to `/usr/bin/swift` when absent. |
| `WENLAN_RB01_CALIBRATION_LOAD_DUTIES` | Comma-separated synthetic-load duty percentages, each `1..=100`, with a total cap of `300`; must be supplied together with the CPU band. |
| `WENLAN_RB01_CALIBRATION_CPU_BAND` | Required `min:max` observed system-CPU percentage band for a calibrated profile; must be supplied together with load duties. Outside the band, the test records a skipped calibration and performs no inference. |

## Product telemetry flag

| Flag | Contract |
|---|---|
| `WENLAN_TELEMETRY_DISABLED` | Set to `1` to make product telemetry unavailable: the daemon reports telemetry as unavailable and does not record or send operation counters, even when persisted consent is enabled. When unset, telemetry remains available only in non-debug builds with an initialized HTTP client. |

## Files mirror flag (storage migration step 3)

| Flag | Contract |
|---|---|
| `WENLAN_FILES_MIRROR` | Set to `0` to stop `files_mirror.rs` from keeping `<data root>/files-mirror/`. When unset, from two minutes after start the daemon brings `files-mirror/files/` up to date at most every two minutes after the database changed (longer when a refresh is slow: at most a tenth of the time goes to refreshing), writing only the files that changed (read from one snapshot of the database, no copy). Once a day, and only after a change, from ten minutes after start, it also copies the database (`VACUUM INTO`), refreshes the files from the copy and rebuild-checks them. The files are never read back, so turning it off loses nothing; `/api/status` then reports `files_mirror.state: off`. |

### Local delivery observations

`GET /api/telemetry` additionally returns nullable `last_delivery`. This is a
process-local observation for the latest batch in the current consent epoch:
`http_accepted` (204), `http_capped` (429), `http_unavailable` (503),
`http_rejected` (other 4xx), `unexpected_http_status` (all other responses),
`transport_error`, or `cancelled`. No attempt is `null`, not success. A pending
count of zero only means the volatile batch was drained/dropped.

The observation is not included in the four-field outbound payload, not persisted,
and cleared on revocation/restart. A late response from an old consent epoch cannot
repopulate it. It contains no IDs, timestamps, body, URL, error details or operation
counts. `http_accepted` proves only the HTTP response, not unique-client attribution,
independent database persistence, installs or real-user usage. The existing desktop
Settings IPC currently projects only consent/availability/pending fields; operators
read this diagnostic through the isolated daemon HTTP API, not the Settings UI.

The hourly clock begins at daemon startup, not at opt-in. Short sessions, restarts,
failed requests and capped responses intentionally lose counters; there is no retry
or disk queue. Search counters also include topic-scoped Brief retrieval, not only
manual search-button actions.

Revocation turns off the live gate even if preference persistence fails. A failed
write is returned as an error, not durable success: an old `enabled: true` file may
be read after restart. After such an error, keep the process disabled (or set the
emergency-off environment flag), resolve the filesystem failure, retry revocation,
and verify the persisted state before restarting. Never describe an errored revoke
as a successfully persisted opt-out.
