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
| `WENLAN_DEV_STATE_DIR` | When set (non-empty), `skill_grow.rs` reads the person's existing AI-tool skill folders from `$WENLAN_DEV_STATE_DIR/ai-tools/home` and `.../ai-tools/config` instead of the real home and config folders, mirroring the app's isolated dev roots, so a dev or scratch daemon never reads real skill files. Unset in normal use. |
| `WENLAN_DEV_SKILLS_ENDPOINTS` | Only read when `WENLAN_DATA_DIR` names a root other than the installed app's own (a test or trial daemon). When set (non-empty) to a base URL, `skill_registry_routes.rs` fetches skills from `<base>/skills-sh`, `<base>/wenlan-site`, `<base>/github-api` and `<base>/github-raw` instead of skills.sh, wenlan.app and GitHub, so a trial daemon can be driven against a local stand-in. The installed app's own run ignores it. Unset in normal use. |

## Rulesync writer flags (UX-SKILL-15..17)

| Flag | Contract |
|---|---|
| `WENLAN_RULESYNC` | Set to `1` to switch every writer to the person's AI-tool folders to the one rulesync writer (`wenlan_core::rulesync_personal::enabled`): rules and skills render through the pinned rulesync into private staging and `rulesync_apply.rs` places them with compare-and-swap, pull-back and journaled restore. Unset (the default while the owner accepts the feature), the older per-tool writer in `skill_grow.rs` stays in charge and `wenlan rules adopt/sync/restore` refuse. |
| `WENLAN_RULESYNC_BINARY` | Path of a local rulesync binary to use instead of `<data root>/rulesync/<pinned name>` (`rulesync_routes.rs`, `wenlan_core::repo_wiki_project_rules`). The file is copied to a private snapshot and must match the pinned version's SHA-256 before it runs (`rulesync_renderer.rs`), so it only chooses where the pinned binary is read from. Unset in normal use. |

## Files mirror flag (storage migration step 3)

| Flag | Contract |
|---|---|
| `WENLAN_FILES_MIRROR` | Set to `0` to stop `files_mirror.rs` from keeping `<data root>/files-mirror/`. When unset, from two minutes after start the daemon brings `files-mirror/files/` up to date at most every two minutes after the database changed (longer when a refresh is slow: at most a tenth of the time goes to refreshing), writing only the files that changed (read from one snapshot of the database, no copy). Once a day, and only after a change, from ten minutes after start, it also copies the database (`VACUUM INTO`), refreshes the files from the copy and rebuild-checks them. At start, when `memorydb/origin_memory.db` is missing or empty and the copy has a manifest, the daemon rebuilds the database from it (`store/restore.rs`) and moves the files it read to `files-mirror/files-restored-<time>/`. Every change to a table the files hold is first appended to `files-mirror/log/` and synced (`store/write_log.rs`, installed through a SQLite auto extension on each connection to the database file); a commit whose log line cannot be written is refused. Each refresh records how far into the log its files go (`files/log-position.json`) and removes the log segments they hold; a restore applies the lines after that position, and at start the last logged transaction is applied when the database lost it. With it off, nothing is logged, refreshed or restored (a missing database with a copy present is logged); `/api/status` then reports `files_mirror.state: off`. |
| `WENLAN_FILES_MIRROR_REFRESH_SECS` | For process tests: seconds before the first `files-mirror/` refresh and the least gap between two refreshes, in place of two minutes (`files_mirror.rs`). `tests/delete_database_rebuild.rs` sets it to `1` so a saved page and memory reach the files within seconds. Unset in normal use. |

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

## Team sign-in (`team_auth.rs`)

Web sign-in on the team listener, with an OpenID Connect provider (Google
today). It is on only when the team's address (`WENLAN_TEAM_URL`, else the
address saved in `team.json`) is `https://` and a provider is configured;
otherwise every `/auth/start`, `/auth/callback`, `/auth/link` and
`/auth/reauth` route answers 404 and one line says why at start.

| Setting | Meaning |
|---|---|
| `WENLAN_TEAM_GOOGLE_CLIENT_ID` | The Google OAuth client id. Beats `team-auth.json`. |
| `WENLAN_TEAM_GOOGLE_CLIENT_SECRET` | The client secret. Never logged. Beats `team-auth.json`. |
| `team-auth.json` | In the data root next to `team.json`, mode 0600 on Unix: `{"google": {"client_id": "...", "client_secret": "..."}}`. An `https://` `issuer` may be added for another OIDC issuer. |

Register `<WENLAN_TEAM_URL>/auth/callback/google` as the redirect URL; Google
accepts a domain name, not an IP address. Login transactions and browser
sessions live in memory, so a restart signs everyone out. Requests to the
`/auth` routes are limited per direct peer IP (a token bucket, burst 30, one
request per two seconds after): behind a reverse proxy all clients share the
proxy's bucket, and there is no trusted-proxy list yet. Re-signing-in
("reauth") needs the provider to return `auth_time`; a provider that never
does cannot complete it.

## Personal cloud (`cloud_gateway.rs`, `cloud_locator.rs`, `cloud_pass.rs`)

`wenlan-server gateway` is the entrance; `docs/personal-cloud.md` says how it
runs. It starts each person's Wenlan with this flag:

| Flag | Effect |
|---|---|
| `WENLAN_CLOUD_ACCOUNT` | This Wenlan is that account's in the personal cloud: the team listener takes only the entrance's pass for it (checked with `cloud-pass.key` in the data folder) and no member key or cookie, and the daemon starts no background work. Set by the entrance, never by hand. |
