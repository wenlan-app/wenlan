# wenlan-relay

The new Cloudflare Worker and submission service name is `wenlan-relay`.
The old `origin-relay` name is retained only when referring to legacy clients
or historical deployment evidence. The separately approved account-subdomain
cutover moved both Workers to `wenlan-app.workers.dev` on 2026-09-10 UTC.
The production gateway origin is `https://relay.wenlan.app`.
This does not update installed desktop binaries or prove submission readiness.

## Local-first reviewer preparation

The intended review path uses Wenlan running on the reviewer's own computer,
with a synthetic local library and normal device consent through this public
relay. It does not require a developer-owned always-on machine or hosted
knowledge runtime. A device must be reachable while its library is queried;
offline and recovery are explicit lifecycle states. Hosted sample-account
experiments later in this document are not prerequisites for this path.

`tests/fixtures/reviewer-api-seed.mjs` prepares synthetic records through the
existing local daemon APIs, not direct database writes. It requires an explicit
non-default loopback endpoint and matching scratch knowledge path, refuses
existing fixture Spaces, and returns the generated IDs. Never point it at a
personal profile. Partial failure leaves the scratch data for inspection and
does not delete, retry or overwrite it. Generated page IDs must be reconciled
with the review cases; the helper does not modify the submission manifest.

This helper does not mint human-presence authorization or reproduce the direct-DB
fixture's confirmed review flags. Do not assume all new pages require review
before querying: the normal-API native MCP lane exercises that behavior directly.
The [reviewer preparation guide](reviewer/README.md) wraps it with an explicit
confirmation and exclusive output receipt via `reviewer/prepare.mjs`. That
command prepares source-linked pages and generates test cases with actual IDs;
it does not alter the portal or the source submission manifest. It requires
Node.js and an already isolated running Wenlan candidate. Neither the helper
nor the command is a published installer, end-to-end ChatGPT pass or approval.

```sh
node --test relay/tests/reviewer-api-seed.test.mjs

# Optional real-daemon check: only a fresh isolated library, no relay enrollment.
WENLAN_NO_AUTOSTART=1 WENLAN_TEST_API_FIXTURE=1 \
  WENLAN_TEST_SERVER_BIN=/absolute/path/to/wenlan-server \
  WENLAN_TEST_FASTEMBED_CACHE=/absolute/path/to/existing/model-cache \
  node --test relay/tests/native-api-fixture-check.mjs
```

Set `WENLAN_TEST_MCP_BIN` to the absolute built MCP binary to also exercise the
three discovered query tools and five cases using generated IDs. This mode uses
authenticated loopback MCP only, not public OAuth or an AI model.
Set `WENLAN_TEST_REVIEWER_PREPARE=1` to exercise the actual preparation CLI and
its generated case receipt before those same real MCP queries.
For an extracted preparation archive, `WENLAN_TEST_REVIEWER_PREPARE_BIN` selects
its absolute `prepare.mjs` entry point; the same real-daemon/MCP checks apply.
The native App lifecycle lane accepts `WENLAN_APP_FIXTURE_MODE=api` to create the
same sample through the App-owned daemon after startup, without the Rust seed
example. It retains the normal isolated bundle/hash/port gates.

The native check retains its scratch path, binary hash and process receipt.
Its source queries report visibility without bypassing page review. A passing
preparation check alone does not make all reviewer query cases ready.

## Production and staging

Deploy the same reviewed source through separate explicit configuration files:

| Environment | Configuration | Current public origin |
| --- | --- | --- |
| Production | `wrangler.standalone.toml` | `https://relay.wenlan.app` |
| Staging | `wrangler.staging.toml` | `https://wenlan-relay-staging.wenlan-app.workers.dev` |

Each service uses its own OAuth KV and script-local `RelayAuthority` namespace.
Do not add cross-script Durable Object bindings, share KV IDs, copy OAuth
records or reviewer secrets, or forward one environment through the other.
The same migration tag/class name does not share storage across Worker scripts.
Staging has no production submission-domain challenge, no sample-account secret,
and no preview URLs. Use only isolated synthetic libraries for staging.
Production credentials must not be accepted by staging. Configuration checks
and the public isolation lane below guard these boundaries; they do not prove
all end-user staging workflows have been exercised.

Use the explicit config rather than relying on named-environment inheritance:

```sh
wrangler deploy --config relay/wrangler.staging.toml --dry-run
# Deploy only the reviewed, verified artifact to the authorized environment.
wrangler deploy --config relay/wrangler.staging.toml
```

The production origin is `https://relay.wenlan.app`, bound directly to the
standalone Worker as a Cloudflare Custom Domain. This does not by itself prove
submission readiness. Preserve all website/mail DNS records during any
separately approved zone migration. Keep the production config's `PUBLIC_ORIGIN`
and custom domain route, App pin, and OAuth/discovery/submission metadata
aligned, then verify client reconsent. Do not introduce a second production
config with the same Worker name and conflicting origin. The current production
App remains pinned to production; adding staging does not introduce an arbitrary
endpoint override or share the installed App's credential store.

To check live separation, add `WENLAN_TEST_ENVIRONMENT_ISOLATION=1` to the
authorized public native-client lane documented below. It uses its own
synthetic production fixture to obtain a valid OAuth access/refresh grant and
device credential, verifies that staging rejects all three, and still runs the
normal production query/revocation checks. It does not use personal libraries,
copy installed credentials, or establish model-mediated routing evidence.

This directory versions the public query forwarding boundary separately from
the unversioned legacy `origin-relay` directory. The standalone authenticated
Worker was deployed on 2026-09-09 with its own OAuth KV and SQLite authority,
without a legacy service binding. This does not replace an already installed
desktop App or prove the complete ChatGPT/Codex submission flow.

The former naming-only compatibility entry forwarded through a Service Binding
to the legacy service. It was NOT the OAuth/query-only gateway and must not be
used as proof of public submission readiness. Its same-name
deployment config, entrypoint and tests have been removed from this standalone
candidate and preserved in the private migration receipt. Removing local files
did not itself change the deployment or any installed client's URL; the later
standalone deployment replaced it. The legacy Worker remains unchanged for
installed legacy clients until their migration is verified.

Only the standalone production config and its generic template may target
`wenlan-relay`. Install and run the local CI suite from the repository root
with Node 24. The lockfile pins the toolchain; no legacy checkout is needed:

```sh
npm ci --prefix relay
npm test --prefix relay
```

`test:unit` and `test:runtime` select smaller lanes. The runtime lane uses local
Miniflare/workerd, not public deployment or a native App. The runner removes
inherited Wenlan live-test opt-ins, serializes test files, and does not select
the opt-in public/native/model `*-check.mjs` probes. Skipped checks are not
passing acceptance evidence; idle SSE abort now has a strict runtime assertion.

The development toolchain pins Wrangler 4.131.1, Miniflare 4.20260730.0,
esbuild 0.28.1 and ws 8.21.0. Tests retain the Miniflare 4 option API; scoped
overrides update its sharp and undici dependencies to 0.35.4 and 7.29.0 for
known vulnerability fixes, and workerd to 1.20260911.1 to support the configured
compatibility date. Wrangler separately brings its own Miniflare version.
Updating these tools requires the local suite again; it does not deploy the
Worker. CI also rejects high/critical npm audit findings, including development
dependencies. A clean audit is not a complete security assessment.

For a focused parsed-configuration check after installation:

```sh
WENLAN_RELAY_TOOLCHAIN="$PWD/relay" \
  node --test relay/tests/deployment-config.test.mjs
```

Run the dependency-free policy tests with Node 24:

```sh
node --test relay/tests/proxy.test.ts
```

For an authorized `tools/call`, an upstream connection rejection before a
response arrives is returned as an MCP tool result with `isError: true` and
actionable local-device guidance. A fresh authorization/lease check must pass
before that result is returned. Revoked, expired, aborted or invalid requests
remain rejected; initialization, tool discovery and session errors retain their
existing transport behavior. The message does not prove a device is powered
off, and never includes raw transport exceptions or private connection details.
This source behavior requires an exact-version deployment and client check
before it can be described as production behavior.

The device Durable Object marks only its internal dispatch rejection; the
central wrapper consumes that marker before the public response. Native peers
cannot provide the marker through the reverse protocol's header allowlist.
Regression checks cover the proxy, session boundary and actual Worker runtime:

```sh
node --test relay/tests/tool-unavailable.test.ts relay/tests/reverse-protocol.test.ts
WENLAN_RELAY_TOOLCHAIN=/absolute/path/to/installed/relay/toolchain \
  WENLAN_NO_AUTOSTART=1 node --test relay/tests/reverse-connection-errors.test.mjs relay/tests/reverse-worker-runtime.test.mjs
```

The connection-error test uses the same Miniflare toolchain as the Worker
integration tests because its source imports Cloudflare runtime modules.

### Native reverse transport

`src/reverse-protocol.ts` and `src/reverse-transport.ts` provide a bounded
transport for an outbound device WebSocket in the current Worker source.
The desktop controller now owns the native Rust socket runtime beside its
protected local MCP child, including connection replacement and bounded recovery.
These layers do not replace OAuth or data-scope checks. The standalone Worker
now supports this transport; that deployment does not update an installed App.
Never deploy `tests/fixtures/reverse-worker.ts`:
it intentionally has no authentication and exists only for local runtime tests.

The v1 wire format accepts only `/mcp` and `/connector-info`, filters headers,
limits requests to 64 KiB and responses to 2 MiB, and permits at most eight
pending calls per connection. Each streamed chunk is at most 16 KiB, with one
sequence-specific credit granted by a consumer pull. A response ends after at
most 4096 chunks or 30 seconds. End may race one final outstanding credit.
Explicit transport cancellation propagates upstream; disconnect or invalid
framing fails pending requests closed. A peer may send `cancel` for a pending
request when local HTTP fails, before or after response headers. It fails only
that request with a generic error, frees capacity and is not echoed. Valid late
frames for bounded cancellation tombstones are ignored; unknown or completed
IDs remain protocol violations. No bodies or credentials are logged or persisted
by this layer.

The existing query/session policy can select a reverse connection through an
explicit trusted adapter. A stored route must have exactly one tunnel origin
or reverse connection ID; missing or ambiguous transports fail closed, with no
fallback. Reverse connection replacement changes the session binding and stops
an in-flight response at the next authorization check. Existing tunnel session
binding bytes remain unchanged. The source Worker supplies only verified
connections through this adapter. Public native-client verification exercises
the deployed path with isolated real data and MCP, as described below.

Native device endpoints in the candidate source are:

- `POST /devices/reverse`: prepare a disabled device with a five-minute
  verification window; only `backendToken` and `space` are accepted. This
  shares the existing `/devices` enrollment quota and creates no OAuth route.
- `GET /devices/reverse/connect`: WebSocket upgrade using the management
  bearer, `x-wenlan-device-id`, and exact `wenlan.reverse.v1` subprotocol.
  The response supplies an opaque `x-wenlan-connection-id`, never a credential
  in a URL. Browser Origin/cookie requests are rejected. Both protected
  connector probes must pass before an atomic, revision-checked activation.
- `GET /devices/reverse/status`: native authenticated readiness query using
  that connection ID in a header. Pending devices cannot approve OAuth.

Authorization records and compare-and-swap operations remain in the single
`wenlan-v1` authority. Socket ownership uses device-named objects in the same
existing namespace, with at most two channels per device: the live connection
and one pending replacement. Other devices cannot fill those two slots. Public
HTTP always enters the central authority; binding-only RPC carries activation
snapshots and current-state checks. This is not a global capacity SLA: existing
enrollment/request quotas and platform resource limits still apply.

A live channel is capped at 16,384 inbound frames per fixed minute,
including ignored late cancellation frames; its in-memory counter resets on
object restoration. Idle verified channels use DO WebSocket hibernation attachments with
only identifiers and a generation, not tokens or knowledge. Credential rotation
and device revocation close owned channels; query checks remain per-request
and per-stream. The alarm sweeps expired channels. Actual eviction/wakeup and
full desktop sleep/wake, restart and prolonged-outage behavior still need native
lifecycle verification.

```sh
node --test relay/tests/reverse-protocol.test.ts relay/tests/reverse-transport.test.ts
node --test relay/tests/reverse-proxy.test.ts relay/tests/reverse-corpus.test.ts
node --test relay/tests/reverse-devices.test.ts relay/tests/connector-check.test.ts
WENLAN_RELAY_TOOLCHAIN=/absolute/path/to/installed/relay/toolchain \
  node --test relay/tests/reverse-runtime.test.mjs
WENLAN_RELAY_TOOLCHAIN=/absolute/path/to/installed/relay/toolchain \
  node --test relay/tests/reverse-worker-runtime.test.mjs
```

The runtime check uses real local workerd and WebSocket messages to verify
incremental SSE delivery and disconnect behavior. It is not a native desktop,
public-network, OAuth, hibernation, sleep/wake or reviewer-availability test.
The full Worker reverse test additionally exercises pending enrollment, explicit
OAuth consent, policy forwarding, session invalidation on reconnect, revocation
and shared quotas, with zero HTTP tunnel calls. Its peer is synthetic, not the
native Rust client. The relay adds a one-second SSE comment keepalive after an
authorization recheck, only at complete LF/CRLF event boundaries and with
downstream demand. This closes the former idle-at-boundary HTTP abort TODO;
the test requires upstream cancellation within four seconds without relying on
native-peer heartbeats. JSON and source data frames are unchanged. Comments
count toward the response byte budget and stop on revocation/cancellation.
Partial events and undecided CR-only boundaries are left untouched rather than
corrupted by a comment. They retain the existing 30-second lifetime cap, with
a separate partial-event runtime test; not every passive close has a four-second
guarantee. Explicit session deletion and device revocation are tested separately.
The candidate enables `enable_request_signal`; a separate direct-socket test
proves that explicit TCP reset cancels upstream within four seconds, without
injecting SSE bytes or relying on the session-deletion path. This does not
establish the same behavior for a passive HTTP connection close. A separate
actual-Worker test with a synthetic peer's one-second SSE heartbeat does cancel
upstream within four seconds. The query-only MCP server now configures rmcp to
frame one-second keep-alives, verified against the real MCP binary. Those are
separate checks, not full native-to-public cancellation evidence.
An unverified socket must never become an active OAuth route.
The native Rust client has exercised the actual local Worker contract, including
stored-profile connection replacement, OAuth, revocation and zero tunnel calls:

```sh
WENLAN_NO_AUTOSTART=1 \
WENLAN_RELAY_TOOLCHAIN=/absolute/path/to/installed/relay/toolchain \
WENLAN_RELAY_CONTRACT_TRANSPORT=reverse-runtime \
  node --test relay/tests/desktop-client-contract.mjs
```

That fixture uses a synthetic local MCP backend, not the full desktop App.
Full native App lifecycle verification remains required. The deployed reverse
path has additionally passed the public native-client protocol lane below;
that receipt does not prove App UI, ChatGPT use or reviewer availability.
The TypeScript and Rust codecs share a checked-in acceptance/rejection corpus
at `tests/fixtures/reverse-protocol-corpus.json`; both language test suites must
pass before changing the wire contract.

The Rust check runs inside the existing desktop test target. Prepare real
sidecars using `scripts/prepare-sidecars.sh` before building this target; a
missing Tauri sidecar is a packaging prerequisite, not a passing codec test:

```sh
WENLAN_NO_AUTOSTART=1 \
  cargo test -p wenlan-app --lib remote_relay::reverse_protocol --offline
```

To exercise the OAuth relay against a real isolated Wenlan database, daemon and
HTTP MCP, run from the repository root with the installed relay Node toolchain:

```sh
WENLAN_NO_AUTOSTART=1 WENLAN_TEST_REAL_RELAY=1 \
  WENLAN_RELAY_TOOLCHAIN=/absolute/path/to/installed/relay/toolchain \
  cargo test -p wenlan-mcp --test reviewer_real_backend --offline -- --nocapture
```

The toolchain path must contain the existing `miniflare` and `esbuild` packages;
this command does not install dependencies. The Rust test owns the synthetic
database and random loopback listeners, then invokes `tests/real-backend-check.mjs`
while they are alive. Only the test tunnel transport maps to loopback; OAuth,
SQLite authority, bearer replacement, session mapping, query wrappers, source
links and daemon reads are real. It checks five positive cases, unauthorized
Space/write denial, offline/recovery, refresh, session deletion and revocation.
Without `WENLAN_TEST_REAL_RELAY=1`, the Rust target checks the real local backend
and MCP only; the Node wildcard suite does not run this combined check.
Neither mode proves a public tunnel, deployed Worker, browser flow or actual
ChatGPT/Codex client use. All data is disposable and no cloud resource is created.

### Persistent synthetic reviewer library

The one-time `seed_reviewer_library` example creates a new isolated library
using the same canonical fixture as the real-backend tests. It refuses an
existing directory (including a partial previous run), never deletes/reseeds
one, and starts no service. It writes `config.json`, `memorydb/`, `pages/` and
an isolated `home/`. Do not invoke it against a personal or installed data root.

```sh
cargo build -p wenlan-mcp --example seed_reviewer_library --offline
cargo build -p wenlan-server --bin wenlan-server --offline
cargo build -p wenlan-mcp --bin wenlan-mcp --offline
target/debug/examples/seed_reviewer_library /absolute/new/reviewer-library
```

Use an existing compatible embedding cache through `WENLAN_TEST_FASTEMBED_CACHE`
when preparing a test library; otherwise normal core initialization can download
its model. This is public model data, not the personal knowledge database.
For a hosted reviewer, preserve its model cache outside disposable test roots.

Run the existing daemon with `WENLAN_DATA_DIR` pointing to that library, `HOME`
(and `USERPROFILE` where applicable) pointing to its `home/`, and an explicit
loopback `WENLAN_PORT`/`WENLAN_BIND_ADDR` including the port. The daemon also
handles home-based compatibility imports, so isolating only its DB is insufficient.
Start the existing MCP binary with the daemon URL, `WENLAN_SPACE=atlas-review`,
`serve --tool-profile query-only`, and a private token file or named token env.
Do not pass the token value in argv. This does not install a supervisor, enroll
a device, start a tunnel, renew its route or activate the reviewer account.

The opt-in POSIX native smoke check seeds a private library, starts the actual
daemon/MCP binaries with a sanitized environment, checks the live pages path,
runs the real OAuth relay cases, stops and verifies both listeners, and repeats
after restarting against the same persisted database:

```sh
WENLAN_NO_AUTOSTART=1 \
  WENLAN_NATIVE_BIN_DIR=/absolute/path/to/built/target/debug \
  WENLAN_TEST_FASTEMBED_CACHE=/absolute/path/to/existing/model-cache \
  WENLAN_RELAY_TOOLCHAIN=/absolute/path/to/installed/relay/toolchain \
  node --test relay/tests/native-reviewer-check.mjs
```

This was exercised on macOS, not a Windows process-lifecycle test or signed
desktop-app test. Stable hosting, tunnel lifecycle, authenticated renewal and
expiry monitoring remain separate deployment gates. Use an approved host and
its existing supervisor rather than treating the temporary smoke driver as a
production service. Never deploy a test-only control endpoint or fake tool data.

`forwardQuery` is called only after a proven OAuth adapter has verified the
token. The grant is trusted adapter output, never client-supplied JSON. It reads
an authoritative connector route on every call, checks user, device, Space,
grant generation, expiry and revocation, and forwards only query operations.
The local backend must run `query-only`, with a separate bearer credential and
the same strict `WENLAN_SPACE` pin. A whole-library grant stores the Space value
`*` instead, and its backend runs `--whole-library` with no Space pin; core
refuses `*` as a Space name, so one Space can never claim it. The client OAuth
token is never forwarded.

### Per-app write access

The user chooses per app, in the desktop approval dialog, between search only
and search and write. The desktop sends it as the optional `access`
(`"query"` or `"write"`) on `POST /pairings/:id/approve`. A missing value means
`"query"` so older desktop builds keep working; any other value is a 400 and
leaves the pairing pending. The OAuth request still has to be exactly
`wenlan:query` (clients do not change). The issued token scope carries the
choice: `wenlan:query`, or `wenlan:query wenlan:write` for a write grant, and a
refresh keeps it even when the client asks for `wenlan:query` alone. Protected
resource metadata keeps advertising only `wenlan:query`: the library publishes
that list as the scopes clients request first, and requesting `wenlan:write`
would be refused at `/authorize`.

Every grant needs `wenlan:query`; it is a write grant only when its scopes also
include `wenlan:write`. Grant receipts and the device grants list carry
`access` (`"query"` or `"write"`; receipts written before it read as `"query"`).
Query grants keep exactly `brief`, `recall` and `get_page_sources`. Write
grants also allow `read_page`, `list_pages`, `capture`, `write_page`,
`rename_page`, `forget` and `delete_page`, under the same `space`/`domain`
argument rule. For a write grant only, the relay sets `x-wenlan-access: write`
on the request it forwards, over both the tunnel and the reverse channel (the
reverse wire protocol allows that one extra request header). The header is not
in the forwarded-header allowlist, so a client-supplied value is never passed
on. Write tool
calls have their own hourly limit (see Limits and request logs).

`verifyConnector` in `src/connector-check.ts` verifies the authenticated local
`/connector-info` contract before a future enrollment handler stores a route.
It rejects an anonymous success, a different Space, standard mode, redirects,
malformed data and responses over 4 KiB, within a five-second deadline.
It is not yet wired into deployed enrollment and does not prove device ownership.
Run its checks with `node --test relay/tests/connector-check.test.ts`.

`src/devices.ts` owns the internal enrollment, authenticated route refresh,
credential rotation and revocation transitions. Enrollment probes the protected
connector contract before storing a route and issues a separate random management
credential, stored only as a hash. The device-derived subject is not an email or
cloud-account identity. Routes expire after 24 hours; management credentials
expire after 90 days and can be rotated before expiry. These access lifetimes
are distinct from the asynchronous physical cleanup described below.

Route refresh authenticates before probing, then rechecks ownership and a
revision in the write transaction. Space/backend-credential changes invalidate
prior approvals; ordinary tunnel URL refresh preserves their scope. Rotation
must not be confused with transport changes: legacy `/devices/refresh` and
`/devices/renew` reject reverse routes before any tunnel probe. Reverse routes
renew through a verified socket activation, not a tunnel update. Credential rotation
invalidates both the old management credential and existing approvals. Revocation
disables the device and its route together. Revocation is idempotent after a
lost response, including after maintenance removes both records. For an existing
device it requires the current management hash, even when the credential has
expired or the device is already disabled; an old rotated token cannot revoke
its replacement. Expired credentials cannot read, approve, refresh or rotate.
If both server-generated device and route IDs are absent, the authority can
confirm terminal denial without authenticating an identity. It does not recreate
either record. The native client clears pending disconnect only after an explicit
successful server response, never on local expiry, 401 or network failure.
The native tunnel health loop now renews the route every six hours. Transient
failures back off for 5/10/20/40/60 minutes, capped at one hour, respecting a
bounded Retry-After. Only an existing enrolled device can renew; no implicit
enrollment or credential rotation occurs. Local revision checks reject a stale
completion after disconnect. Non-retryable authorization/profile failures stop
the owned transport and surface an error. Wall/monotonic divergence can trigger
early renewal, but failed retries retain monotonic backoff: sleep/wake recovery
can therefore wait up to one hour. Packaged desktop suspend/resume remains an
unverified gate, not something established by deterministic policy tests.
Native storage is implemented below;
the complete desktop consent/recovery UX remains open. The actual HTTP adapter now
implements parsing, CSRF and initial limits.

`src/pairing.ts` implements short-lived single-use browser/desktop pairing.
Only a validated server-owned OAuth intent can start it. The browser secret
is distinct from the visible pairing ID and only its hash is persisted.
Desktop approval binds the current device authorization generation, credential
expiry and explicit Space consent; consumption rechecks the route in the same
transaction. It is not a replacement for an OAuth protocol implementation.

Both state modules use the same serializable durable transaction contract.
Cloudflare's SQLite-backed Durable Object storage implements that contract
directly; Workers KV does not. The local workerd test uses the real storage API
to verify restart persistence, concurrent one-time consumption, rollback and
device revocation. Its fixture has privileged test operations and MUST NOT be
deployed or imported by any production entrypoint.

```sh
node --test relay/tests/devices.test.ts relay/tests/pairing.test.ts
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  node --test relay/tests/pairing-runtime.test.mjs
```

The runtime test requires existing `miniflare` and `esbuild` packages from the
selected toolchain. It uses loopback listeners, a temporary SQLite directory,
synthetic data and denied outbound fetches; it disposes its runtime and removes
its own temporary files. It does not provision cloud storage or prove deployed
OAuth behavior. Missing tooling is a failed gate, not a skipped success.

## OAuth integration

`src/oauth.ts` integrates the pinned Cloudflare OAuth provider package rather
than implementing OAuth token cryptography. Install this isolated package with
`npm ci --prefix relay --ignore-scripts --no-audit --no-fund`. The lockfile owns
the exact package version/integrity; this does not upgrade the existing App or
Wrangler installation.

The adapter stores a validated authorization request, starts the existing
pairing flow, and completes authorization only after single-use consumption of
desktop-approved pairing. It uses a fixed HTTPS resource/issuer, S256 PKCE,
15-minute access tokens and refresh tokens that lapse after 30 days without
use. Each refresh restarts that window, never past the authorization's 90-day
end. DCR registrations use an explicit `90 * 24 * 60 * 60`-second TTL from
registration; a successful token request in its second half renews it for
another 90 days. Registrations made before the 1.x library carry no renewal
stamp and keep their original schedule. Once a registration expires, the old client must re-register and obtain fresh desktop
consent before it can authorize or refresh, even when its authorization grant is
still retained. The runtime test models expiry by removing the exact synthetic
client KV key; it does not establish a 90-day wall-clock expiry receipt. Existing
non-expiring deployed clients require a separately approved inventory and
migration, and are not claimed to be fixed here. DCR is the tested registration
path; CIMD is deliberately not advertised until its required runtime flag and
actual client flows are verified.

The protected handler uses the library's token summary for current scope and
expiry instead of authorization-time props. Code exchange/refresh and queries
read current device authorization state. An ordinary expired tunnel can still
renew OAuth credentials, but no query is forwarded through an expired route.
Revoked devices or changed scope generations cannot mint new credentials.
The SQLite consent and grant records cap authorization at 90 days, and the
device's management credential caps it too; refresh and every query check that
end directly rather than waiting for the cleanup sweep. Later access requires a
fresh consent flow even if a library refresh token remains.
The library revokes a grant and its tokens whenever this check answers
`invalid_grant`, so the check uses it only for permanent denials.

`src/grants.ts` adds atomic authorization-code claims after the library verifies
PKCE, client and resource. A regression exposed two successful simultaneous
exchanges through the provider's KV read/modify/write path. Only one durable
claim may now succeed, and another valid claim revokes the receipt. Queries,
stream chunks and refresh check that receipt. An issuance failure after claim
requires fresh pairing, not reuse of the consumed code. The provider's normal
token revocation/replacement is additionally rechecked through `unwrapToken`;
its distributed KV visibility remains a deployed verification gate.

Reauthorization replaces the current consent record for that device/client
before the provider issues another code. Queries, refresh and streams require
the receipt to refer to that current consent. Restoring old synthetic KV records
after reauthorization cannot restore access. If issuance fails after replacement,
the user must pair again; the previous connection is not silently restored.

The actual Worker prevents concurrent token exchanges and pairing completions
from entering the provider's KV mutation flow. A busy token endpoint returns
OAuth `temporarily_unavailable` (503) with `Retry-After: 1`, not a cached token
response or an unbounded queue. MCP queries and device revocation remain
independent. This is a single-authority v1 throughput tradeoff; actual client
retry handling and production capacity still need verification.

```sh
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  node --test relay/tests/oauth-runtime.test.mjs
```

This executes the real OAuth library against local KV and a SQLite Durable
Object, including enrollment and the pairing state machine. It tests discovery,
redirect/resource/PKCE rejection, replay and concurrent code exchange, refresh,
expired token rejection and device revocation. The fixture uses synthetic
connector responses and returns a browser secret as JSON for the test driver;
that fixture is NOT a deployable/public HTTP flow. Production must use a
Secure/HttpOnly cookie, CSRF defenses, bounded bodies and abuse controls.

The provider currently expects a dedicated `OAUTH_KV` namespace in addition to
the strongly consistent device/pairing authority. No cloud namespace has been
provisioned by these tests. Local KV does not prove distributed cache/revocation
timing; measure that on the deployed candidate. The strong replay receipt
does not establish production consistency or physical deletion deadlines.

Native `GET /grants` lists only the authenticated device's grants, in bounded
25-item pages with an opaque grant-ID cursor. `POST /grants/{id}/revoke` commits
authoritative denial before deleting provider tokens. Both use the separate
management bearer and `x-wenlan-device-id`; browser Origin/cookie requests are
rejected. Neither request JSON nor a path grant ID establishes ownership.
Other clients on the same device remain authorized. Failed provider cleanup
returns 503 with `revoked: true, cleanupPending: true`, while access remains
denied; repeating the operation retries cleanup. The authority alarm also retries
automatically. The desktop list/disconnect UI is now implemented in source;
live public and installed-App behavior remain unchecked.

```sh
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  node --test relay/tests/grant-management-runtime.test.mjs
```

The store must supply fresh authorization state, not an eventually consistent
cached routing snapshot. Disabled or expired connectors cannot be queried.
Quick-tunnel HTTPS origins are parsed strictly. Redirects are not followed;
cookies, identity headers and raw backend errors are not forwarded. Request
bodies are bounded to 64 KiB and 10 seconds; upstream requests/streams time out
after 30 seconds. Expiry and revocation are rechecked after body upload.

`src/sessions.ts` is mandatory in the production OAuth adapter. It assigns an
independent 256-bit public session ID at initialization and stores its binding
to OAuth grant/client, device, Space, authorization generation and backend route.
Public IDs cannot be substituted with raw backend IDs. The same grant can
continue after token refresh; a different grant/client cannot read, resume or
delete the session. A backend ID can only be claimed once while its mapping is
active. Session mappings expire after 24 hours; tunnel/credential changes require
reinitialization. DELETE and backend 404 deactivate mappings. Backend 400/404/405
remain sanitized protocol statuses so clients can recover or reinitialize.

Active upstream requests and response streams recheck authorization before each
delivered chunk and every second while idle, with a two-second total check
deadline. Revocation, unavailable authorization storage, client cancellation,
token expiry or the 30-second request deadline cancels upstream. Responses are
capped at 2 MiB. Local device/session revocation tests terminate idle streams in
about one second; a stalled store terminates them in about three seconds. These
measurements are local, not production guarantees or distributed KV bounds.
Session/receipt expiry denies access immediately; asynchronous cleanup is separate.

The current local `wenlan-mcp serve` uses rmcp1.5's default five-minute idle
session timeout. The SDK's HTTP service removes manager entries after the
session task exits, including abandoned initialization. The isolated native
`session_retention` test exercises both abandoned and fully initialized idle
sessions with the timeout shortened only in the fixture; it also asserts the
production SDK default remains300seconds. This is not an active-session count
limit, installed-App proof or a five-minute production timing measurement.

Malformed initialization is a separate path: rmcp1.5 may allocate a manager
entry before rejecting a message without starting its normal cleanup task.
The query-only native HTTP entry therefore validates sessionless POST bodies
with rmcp's own typed InitializeRequest parser before passing them to the SDK.
Bodies are limited to64KiB and five seconds. Invalid, oversized and stalled
initializations cannot allocate SDK sessions. The outer bearer/Origin gate runs
first. Standard mode is unchanged; do not extend this protection to old installed
binaries or standard-mode endpoints without separate evidence.

```sh
node --test relay/tests/sessions.test.ts relay/tests/grants.test.ts \
  relay/tests/stream-revocation.test.ts
```

## Limits and request logs

`RelayAuthority.limit` admits each request before it reads the body. The key
follows what the relay can tell about the caller, because Claude, ChatGPT and
other AI apps call from their own shared servers, so many people share one
address:

| Request | Limit |
| --- | --- |
| Every request | 600 per minute in total |
| Anonymous (no credential) | 120 per minute per address |
| `/mcp` with a bearer token | 120 per minute per OAuth grant, after the token is verified |
| `/mcp` `tools/call` of a tool outside the query set, on a write grant | 60 per hour per OAuth grant (`grant-write-hour`), in addition to the per-minute limit; search-only grants never reach it |
| Device API with a device credential | 120 per minute per device |
| Credentials the relay rejects | 60 per minute per address; each credentialed request counts until it is verified and is then refunded, so concurrent guesses stay bounded |
| `/devices`, `/devices/reverse` (turning Web access on) | 10 per hour per address, 100 per day in total |
| `/oauth/register` (client registration) | 60 per hour per address for an allowlisted AI app, 20 for other clients, 250 per day in total |

Registration is limited after its bounded body is read, so the redirect host can
choose the budget. That host is only a claim, so it selects a per-address
budget rather than a shared per-app one that anyone could exhaust. A request
refused by its hourly window never spends a daily one.

Each request to the central authority writes one JSON line,
`{"event":"relay_request",...}`, with `route` (identifiers replaced by `:id`),
`method`, `status`, `ms`, `who` (`anonymous`, `grant` or `device`), `limit`
(the window that refused it), `app` (allowlisted AI app name or `other`, on
`/authorize` and `/oauth/register`), `device` (12 hex characters of the device
identifier's hash) and `error` (the error type of a 503). A 429 without `limit`
came from the per-minute grant or pairing-lookup limit; the write-call limit
logs `limit: "grant-write-hour"`. Lines never contain addresses,
tokens, Space names or bodies; `tests/limits-logs-runtime.test.mjs` checks this.

Workers Logs keep these lines; invocation logs stay off so Cloudflare does not
record request URLs or headers. Read them in the Cloudflare dashboard (Workers,
`wenlan-relay`, Logs), or stream new ones with
`npx wrangler tail --config relay/wrangler.standalone.toml --format json`.

## Retention and cleanup

The standalone authority uses `src/bounded-store.ts` for every application
transaction, including cleanup: at most 8192 application records, with a 32 KiB
UTF-8 JSON limit on inserted/updated values. The capacity counter and cleanup
cursor are reserved control records outside that count; rate-limit SQL rows
and provider OAuth KV have separate limits. This is an initial engineering
capacity boundary, not an OpenAI requirement, pricing guarantee or launch SLA.

Record accounting commits or rolls back with the underlying serializable
transaction. At capacity, existing reads/updates, revocation, and deletion
remain possible; net-new records are refused atomically. Partial device/route
or session/reverse-map writes cannot commit. The HTTP enrollment/authorization
paths return a sanitized 503 on capacity exhaustion. Session initialization may
already have contacted the backend before a failed mapping commit, so the local
MCP's own session lifetime remains an independent resource-control requirement.

First use without a counter counts existing records in bounded 128-record
pages. An oversized legacy store or invalid counter fails closed rather than
silently discarding records. Deployment must begin with the approved empty
standalone namespace; importing existing state requires explicit reconciliation.
Do not bypass the adapter for administrative writes or manually reset its
counter. Unknown records count toward capacity and are not automatically purged.

```sh
node --test relay/tests/bounded-store.test.ts
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_NO_AUTOSTART=1 node --test relay/tests/capacity-runtime.test.mjs
```

The existing authority alarm runs `src/cleanup.ts`, scanning at most 32 stored
records per invocation using a durable cursor. It reschedules independently of
request-rate buckets, including after eviction, and stops when no owned records
or rate buckets remain. Full-scan latency depends on record count and backlog;
these local results do not establish a fixed production deletion deadline.

- Expired five-minute pairing transactions, request snapshots and their bindings
  are deleted; current-consent pointers are deleted after their 90-day expiry.
- Revoked/expired devices and their protected route credentials are deleted.
  An expired tunnel route is retained while its management credential can renew
  it, so temporary offline status does not force re-enrollment.
- Inactive/expired 24-hour session mappings and dangling reverse claims are
  deleted transactionally. Cleanup cannot delete a replacement session's claim.
- Invalidated grants are denied before token cleanup. Receipts remain as replay
  tombstones through both receipt and current-consent lifetimes, even after
  successful token deletion. Expired tombstones are deleted only after cleanup
  succeeds and their consent is no longer issuable.

At most one provider cleanup batch runs per scan. The pinned provider's public
`revokeGrant` helper gets a restricted KV view: at most 12 KV operations per
invocation, four keys per list, and no new operations after 1.5 seconds. The
outer wait fails closed after two seconds. This does not cancel an already
issued KV operation. Partial batches retry on the next eligible scan; storage
failures back off from one minute up to one hour, with durable pending state and
no stored raw exception. Alarms construct provider helpers directly, without
requiring a preceding OAuth HTTP request.

This covers authority records and explicit/device-invalidated grant cleanup,
not a universal provider-KV purge. DCR registrations use the 90-day TTL
described above; the pinned provider writes unexchanged code grants with a
600-second KV TTL and access-token records with their token TTL. Each refresh
moves the refresh grant's KV expiry to 30 days later, never past the Wenlan
grant end, so an unused grant expires on its own. These configured expirations are not measured production deletion receipts.
Legacy client migration, the provider's internal replacement cleanup,
eventual-KV reappearance, quota/backlog alerts and deployed retention
measurements remain review gates.
Do not promise finite wall-clock physical deletion during a storage outage.

```sh
node --test relay/tests/cleanup.test.ts relay/tests/cleanup-kv.test.ts
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_NO_AUTOSTART=1 node --test relay/tests/maintenance-runtime.test.mjs
```

The runtime test delivers a real workerd alarm, persists its SQLite cursor and
alarm across restart, and exercises the actual pinned OAuth library against 25
synthetic KV token records. Its privileged seed/inspection controls are test-only.

## Integration gates

`src/worker.ts` is now the standalone source entrypoint. It owns the
`RelayAuthority` SQLite Durable Object and calls the OAuth and device/pairing
modules directly; it never imports the compatibility entry or old relay.
It requires `PUBLIC_ORIGIN`, dedicated `OAUTH_KV`, and `AUTHORITY` bindings.
The production `wrangler.standalone.toml` binds provisioned resources and the
verified service origin. The first standalone deployment applied migration
`wenlan-authority-v1`; cloud metadata confirmed `RelayAuthority` and the dedicated
OAuth KV, with no service binding. Public OAuth discovery returned200 and
unauthenticated MCP returned401. These checks do not prove authenticated tools,
reviewer availability, desktop migration or target-client compatibility.

`wrangler.standalone.template.toml` describes the standalone package with a
synthetic origin and an explicit `APPROVAL_REQUIRED` KV placeholder, not a real
namespace ID. It has no service binding. Do not deploy this template: approved
provisioning, the verified namespace ID/public origin, migration review and the
remaining release gates are required first. Its SQLite migration creates
`RelayAuthority`; its Data rule packages the existing PNG as a binary module.

The standalone entry supports `GET /.well-known/openai-apps-challenge` on
the exact configured HTTPS `PUBLIC_ORIGIN`. Set `DOMAIN_VERIFICATION_TOKEN`
only to this plugin draft's exact public domain-verification token after
approved deployment. The response is plain text with no added newline or
JSON wrapper, no-store and nosniff. Missing, empty, overlong or whitespace/
control-containing configuration returns404. Other methods return405. The
endpoint needs no OAuth login, DO/KV state or backend connection; it proves
domain control only, not MCP availability, user identity or review readiness.
Never configure a management, OAuth or local MCP bearer as this public value.

Follow the portal's actual challenge rather than generating a local token.
Its Challenge Base URL must use the MCP host or permitted parent host; paths
do not create separate verification slots. One hostname serves one exact
token, not a list for multiple plugins. Verify the public response, then the
portal's successful verification state after deployment. Removing/replacing
this value is a deployment change. No real token is included in the template,
and source support/local fixtures do not mean this draft is domain-verified.
Official procedure: https://developers.openai.com/plugins/deploy/submission#domain-verification.

Validate packaging without uploading or provisioning resources, using an
already installed Wrangler (verified with 4.78.0):

```sh
WRANGLER_SEND_METRICS=false WRANGLER_LOG_PATH=/tmp/wenlan-packaging.log \
  /path/to/existing-wrangler-project/node_modules/.bin/wrangler deploy \
  --config relay/wrangler.standalone.template.toml --dry-run \
  --experimental-autoconfig=false --outdir /tmp/wenlan-standalone-package \
  --metafile /tmp/wenlan-standalone-package/meta.json

WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_RELAY_BUNDLE_DIR=/tmp/wenlan-standalone-package \
  WENLAN_NO_AUTOSTART=1 node --test relay/tests/worker-runtime.test.mjs
```

The optional bundle directory makes the existing flow test load Wrangler's
emitted `worker.js` and PNG module without rebundling. It checks enrollment,
consent, OAuth exchange, query forwarding, revocation and exact icon bytes with
synthetic upstream responses. Without that variable the normal source-bundling
test remains unchanged. Both paths use local ephemeral KV/SQLite bindings;
neither proves provisioned bindings, production migrations or real clients.

Set `WENLAN_TEST_REAL_PAIRING_EXPIRY=1` on the Worker test to exercise the
five-minute pairing lifetime with actual elapsed time. The opt-in run waits
301 seconds and verifies that the same cookie changes from pending to an
unavailable response and terminal page. Without this flag the slow case is
explicitly skipped; pending, cancelled, replayed and forged-cookie cases still
run. Terminal responses disable stale-page actions instead of implying that
approval is still pending.

The explicit recovery lane loads the same frozen Wrangler artifact, with
persisted local KV and SQLite, and restarts workerd between pending pairing,
authorization, querying and revocation. It verifies that pending consent and
authorized sessions survive, while revocation remains enforced and refresh is
denied after restart. All upstream responses are synthetic and intercepted.

```sh
WENLAN_NO_AUTOSTART=1 \
  WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_RELAY_BUNDLE_DIR=/absolute/path/to/frozen-package \
  node --test relay/tests/recovery-runtime-check.mjs
```

Preserve the bundle, its binary modules, matching production config and exact
source before deployment. A recovery upload uses the frozen entry with
`wrangler deploy /absolute/path/to/frozen-package/worker.js --no-bundle --config
/absolute/path/to/preserved-config.toml`; validate with `--dry-run` first. Keep
the same authority class, migration tag, namespace bindings and public domain
challenge. This restores code/configuration, not a point-in-time data backup.

`src/http.ts` owns native device enrollment/refresh/rotation/revocation and
authenticated pairing inspection/approval. Browser enrollment requests are
rejected. Browser pairing uses a `__Host-` Secure/HttpOnly/SameSite=Lax cookie;
completion/cancellation require the exact Origin, a consistent Sec-Fetch-Site
when present, and JSON POSTs. The secret is never in HTML or response JSON.
Authorization redirects to the stable `/pairing` page so reload does not create
a new pairing. The page uses the existing bitmap icon, self-hosted CSS/JS,
strict CSP and no-referrer headers. The native App pairing UI is implemented in
source; public and packaged-App verification remain separate gates.

The Worker bounds URLs/headers and buffers at most 16 KiB for control/OAuth
requests or 64 KiB for MCP, with a five-second body deadline. A single v1
authority serializes rate-counter changes in SQLite. Initial limits are 600
requests/minute globally, 120/minute per hashed connecting IP, 3 enrollments/hour
and 10 DCR registrations/hour per IP. Retry-After uses the actual blocked window.
Expired rate buckets are removed in bounded alarm batches. These are provisional
limits: account for shared ChatGPT/Codex egress IPs and measure production
capacity before deployment. They are not a global availability guarantee.
Public MCP requests with an Origin header must match the configured public
origin; normal server-to-server requests without Origin remain supported.

Run the actual-entry integration test with the same local toolchain:

```sh
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  node --test relay/tests/worker-runtime.test.mjs
```

This entry test uses only synthetic outbound connector responses. It checks
real HTTP authorization/cookies/CSRF/device consent, token forwarding and denial,
body bounds and rate limiting, and asserts the bundle contains no legacy
dependency. Real clients, public network availability and the deployed version
remain separate gates.

This is **not** a submission-ready deployed endpoint. Still required: reviewed
production provisioning/configuration; desktop pairing UI and storage platform validation;
desktop per-grant disconnect controls; full provider-KV retention and cleanup capacity;
distributed OAuth KV consistency measurements; production capacity/abuse policy;
real MCP/daemon and public domain/metadata verification; reviewer hosting;
policy/demo assets; independent integrated review and actual ChatGPT/Codex flows.
Passing local tests is not evidence of public availability or complete isolation.

## Native desktop client

`app/src/remote_relay.rs` is the typed Rust control-plane client for the new
service. It covers enrollment, route refresh, credential rotation, device
revocation, pairing inspection/explicit approval, paged grant listing and
per-grant revocation. Its production target is fixed HTTPS, redirects are
disabled, credentials are sent only in their intended headers/body, errors omit
remote response details, and JSON responses are bounded to64KiB. Device and
backend credential Debug output is redacted. It does not obtain or persist
the ChatGPT client's OAuth token.

The source `remote_access.rs` now uses this client for required enrollment or
refresh, with no legacy registration or direct-tunnel fallback. Initial and
MCP-only restart commands use `query-only`, strict `WENLAN_SPACE`, and a separate
backend token passed in the child's named environment variable, not argv. A
protected connector-info check plus anonymous denial precedes tunnel startup.
Only a persisted, explicitly enabled consent profile can resume on App startup;
the legacy daemon boolean and capability URL are not imported as authorization.
Startup cleanup runs independently of daemon health, file watching and initial
sync. A disabled profile with a device retries pending revocation; an expired
enabled device is disabled and revoked, not automatically re-enrolled. A resume
ticket waits for daemon health and rechecks profile revision and controller
generation, so a delayed startup cannot override a later stop or settings edit.
Completion emits another native status event after durable cleanup for the
settings panel to refresh. The dedicated controller mutex also spans owned
process cleanup, preventing a new start from reusing ports while an earlier
stop still scans them. The AppState RwLock is not held across this cleanup.
Off persists intent, stops owned processes and then attempts device revocation.
Failed remote revocation keeps the management credential for recovery. Late
enrollment completions cannot overwrite a disabled or newer profile; unsuccessful
persistence attempts revoke the new device, and the caller closes the backend.
Lost enrollment replies or failed orphan revocation still require lifecycle
recovery/retention validation, not an assertion of guaranteed immediate deletion.

`app/src/remote_relay/store.rs` uses atomic versioned profiles and a file lock.
Unix directories/files are 0700/0600; data is **not encrypted at rest on Unix**.
Windows uses user-scoped DPAPI without plaintext fallback; Windows execution
is still unchecked. The frontend profile view omits management/backend secrets.
`configure_remote_access` accepts only an existing Space and does not enable
access; enable requires its current revision. Scope changes cannot discard a
pending device revocation. File corruption fails closed instead of resetting
to an enabled default.

The frontend panel now selects an existing Space, requires explicit consent,
inspects a pairing request and then explicitly approves it. Native approval
re-reads the server intent and local revision before posting. A local plugin
installation no longer appears as proof of a web connection. The panel lists
device grants with revoke/retry controls, separates token cleanup from denied
access, and awaits disconnect before reconnect; no fixed-delay restart. Failed
Space/settings reads preserve a stop-access action. English, Traditional and
Simplified Chinese text is included; isolated browser fixture tests are not
evidence of native IPC, public OAuth or installed-App operation.
Do not release this intermediate branch or claim installed-App migration.
Periodic renewal and startup recovery are wired and locally tested, but packaged
sidecars, actual startup IPC, storage-failure stop intent, and real desktop
sleep/wake remain open. No installed App or public deployment has been changed
by these source edits.

```sh
WENLAN_NO_AUTOSTART=1 TAURI_CONFIG='{"bundle":{"externalBin":[]}}' \
  cargo test -p wenlan-app --lib remote_ --offline
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_NO_AUTOSTART=1 node --test relay/tests/desktop-client-contract.mjs
```

The first command tests native storage/client/startup boundaries without
packaged sidecars; its ignored cross-language case is exercised by the second command.
That macOS/POSIX test lane starts local workerd with the actual Worker entry,
bridges loopback HTTP to the configured issuer without rewriting payloads, and
executes the Rust test through a bounded owned Cargo process group. It covers
enrollment, refresh, real-library OAuth pairing/code exchange, MCP initialization,
grant revocation and credential/device revocation against synthetic local data.
Only the backend connector response is synthetic. It creates no cloud resources
and does not test real packaged sidecars, ChatGPT/Codex UX or production TLS.

After explicitly building the current `wenlan-mcp` binary, run the separate
native sidecar contract with `WENLAN_TEST_MCP_BIN` set to its absolute path:

```sh
WENLAN_NO_AUTOSTART=1 WENLAN_TEST_MCP_BIN=/absolute/path/to/rebuilt/wenlan-mcp \
  TAURI_CONFIG='{"bundle":{"externalBin":[]}}' \
  cargo test -p wenlan-app --lib \
  remote_relay::runtime::tests::actual_sidecar_uses_child_environment_and_protected_contract \
  --offline -- --ignored --exact
```

This uses the App's actual argument builder, synthetic native credentials and
a child-only environment. It verifies the protected contract, anonymous MCP
401, initialization, three-tool inventory and failure on a missing token env.
It does not run Tauri's packaged sidecar launcher, cloudflared or a real daemon.

## Sample-account authorization

`src/sample-account.ts` implements the bounded owner-independent reviewer
identity. The Worker now integrates it through an optional server-owned
`SAMPLE_ACCOUNT` JSON binding. No such binding is configured in the deployment
template and no hosted reviewer account has been activated. Missing, malformed,
expired or wrong-resource bindings do not expose the sample-login page.

The operator-only `issueSampleAccount` authenticates an existing device and
returns a fresh 256-bit random password plus its bounded account configuration.
It does not create or modify device/route records. Only a dedicated synthetic
library may be provisioned; the code cannot establish whether arbitrary library
content is genuinely synthetic. The account is pinned to its device credential
hash, route generation, Space and exact HTTPS MCP resource, with a maximum
30-day lifetime no longer than the device management credential. Passwords are
generated, not human-chosen; SHA-256 verification here is a high-entropy secret
scheme, not a password-storage design for user-selected passwords.

For a fresh dedicated synthetic device, the operator can instead prepare the
binding offline from the unmodified successful `POST /devices` response using
`relay/scripts/prepare-sample-account.mjs`. No direct DO access or special HTTP endpoint
is needed. This path pins generation zero and the management-credential hash;
the normal login still checks both against the live authority. Preparation is
not remote authentication, data classification, account activation or deployment.
Do not use an old, rotated or scope-changed device response. A forged or stale
file cannot establish live authorization merely by producing a JSON binding.

After the synthetic backend and enrollment have been approved and verified:

1. Store the fresh enrollment response in an owner-only regular file (mode600).
   Keep the connector bearer and management credential out of terminal output,
   URLs, shell arguments, commits, public artifacts and model context. Do not
   automatically repeat an enrollment whose response was lost.
2. On macOS/Linux with Node24, use the offline command below. The output must be
   a new directory under an owned parent that is not writable by other users.
   The explicit flag is an operator declaration, not a synthetic-data scanner.
3. Inspect the resulting identity, Space, resource and expiry locally. The
   command creates mode700 output and separate mode600 `sample-account.json`
   and `reviewer-password.txt`; it never prints their values or overwrites an
   existing directory. If writing fails partway through, retain and inspect
   that partial directory rather than rerunning into it.
4. Only after deployment approval, install `sample-account.json` as the
   `SAMPLE_ACCOUNT` secret using the approved deployment tooling. Do not put
   it in vars, Git, frontend assets or logs. Use the separate password only in
   the review credential field and the actual sample login form.
5. Prove login against the live device and all three real tools. Keep the route
   renewed while the review is in progress; device credential rotation or a
   Space/backend-credential change invalidates this configuration. Its expiry
   is the earlier of the enrollment credential expiry and30days from preparation.
   Expiry/renewal monitoring and always-online hosting are separate gates.

```sh
node relay/scripts/prepare-sample-account.mjs \
  --enrollment /private/operator/enrollment.json \
  --space atlas-review --resource https://YOUR_APPROVED_RELAY_HOST/mcp \
  --output /private/operator/prepared --synthetic-library
```

The POSIX file-permission check intentionally fails closed on Windows; run this
operator utility on the approved macOS/Linux host. This is not a restriction
on Windows end-user pairing. Windows ACL-based preparation is unimplemented.

For an approved synthetic reviewer host, `relay/scripts/renew-sample-route.mjs`
performs one authenticated route renewal. Supply the original connector JSON
(`tunnelOrigin`, `backendToken`, `space`), original enrollment response and
prepared account binding as owned mode600 regular files. The explicit relay
origin must exactly match the account's HTTPS MCP resource. Only the new
canonical quick-tunnel origin is a command-line value; no credentials belong
in arguments. It does not edit those files or start/restart the tunnel.

```sh
node relay/scripts/renew-sample-route.mjs \
  --enrollment /private/operator/enrollment.json \
  --connector /private/operator/connector.json \
  --account /private/operator/prepared/sample-account.json \
  --relay-origin https://YOUR_APPROVED_RELAY_HOST \
  --tunnel-origin https://YOUR_NEW_TUNNEL.trycloudflare.com
```

The command uses `POST /devices/renew`, not unrestricted `/devices/refresh`.
The server authenticates the existing management credential, requires an
expected consent generation, and rejects a changed generation, Space or
backend credential before probing. It rechecks ownership and revision in the
write transaction. Ordinary desktop refresh remains compatible. Older Workers
without the new endpoint reject the request; there is no downgrade fallback.
Success preserves the original credential expiry, sample-account expiry and
consent generation while renewing the 24-hour route lifetime.

Exit0 confirms one renewal; exit1 means renewal was not confirmed. A timeout
or lost response does not prove failure or an offline route. Output contains
no supplied values or raw server errors. Requests have a ten-second deadline,
responses a 4096-byte bound, and redirects are not followed. There is no
automatic retry, enrollment, rotation, account provisioning or scheduler.
An approved existing host supervisor must renew before route expiry (the
native App uses six hours), bound transient retries and alert on rejection or
credential/account expiry. That supervisor, public tunnel lifecycle and
always-online hosting remain separate unimplemented deployment gates.
Never change Space/backend credentials to keep a stale reviewer binding alive;
that requires new consent and separately prepared review credentials.

`approveSamplePairing` checks that account, the browser's live pairing secret,
the displayed client/resource/Space and explicit consent. It invokes the same
`approvePairing` used by the native device flow. Rotation, revocation, expiry,
scope changes and cancellation still prevent approval, including races before
the approval write. Sample passwords do not authenticate device-management or
MCP requests. Existing OAuth code/token/grant paths must still be used afterward.
Account expiry stops new approvals; already issued grants remain subject to
normal OAuth expiry and device/grant revocation, not a new account-wide session
mechanism. Disconnect/revoke the sample device to terminate existing access.

Run the bounded core tests:

```sh
WENLAN_NO_AUTOSTART=1 node --test relay/tests/sample-account.test.ts
```

For a configured account, `/pairing` offers `/pairing/sample`. The form names
the client, Space and query permission, and requires explicit consent. Its JSON
POST requires the live HttpOnly pairing cookie, exact Origin and same-origin
Sec-Fetch-Site when supplied. The normal 16-KiB control-body limit and five-second
read deadline apply. The authority additionally caps sample-login POSTs at 10
per hashed peer per minute and 60 globally per minute, including failed attempts.
Username/credential errors are generic; the browser clears the password field
after a request. It does not receive the configured account or its credential
hashes. Issuance uses the existing serialized OAuth completion and token paths.

Keep account bindings and passwords out of HTML, URLs, model tools, logs and
checked-in configuration. Only the normal password input on the authorization
page may receive the review password. Configure the binding as a deployment
secret only after approved provisioning and review, never as a public asset.

```sh
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_NO_AUTOSTART=1 node --test relay/tests/sample-login-runtime.test.mjs
```

The actual Worker/SQLite/KV test covers login, consent, real-library code/token
issuance, protected query forwarding, device revocation, invalid bindings,
cookie/CSRF checks, and peer/global admission. It now enrolls using the real
`POST /devices`, prepares the account offline, and reloads the configured Worker
without injecting authority records or exposing a privileged fixture route.
Its connector data is synthetic;
it does not prove actual review data, public client UX or TLS availability.

An attended five-minute loopback-only browser fixture is available separately:

```sh
WENLAN_RELAY_TOOLCHAIN=/path/to/existing-wrangler-project \
  WENLAN_NO_AUTOSTART=1 node relay/tests/sample-login-preview.mjs
```

It prints only temporary synthetic login credentials and stops on interrupt or
timeout. The local HTTP bridge rewrites only its own Origin and retains Secure
on the initial `__Host-` cookie. Loopback remains a test presentation, not
production TLS evidence. Never deploy the preview or maintenance fixture.
For a stale-page check, `WENLAN_PREVIEW_CANCEL_PAIRING_MS=15000` opens the device
pairing view and cancels only its own fixture after 15 seconds. Continuing then
shows the unavailable status and disables obsolete actions. This browser flow
was exercised in Chrome; it does not substitute for live sample-account login.

Verify a real isolated database/daemon/MCP and provision approved always-online
hosting; do not substitute mock responses for the reviewable product. Neither
these local tests nor the configuration type prove a publicly usable review login.

## Public native-client verification

This explicit live lane creates a new isolated synthetic library, starts the
real daemon/MCP binaries and the selected private transport, enrolls one device
through the deployed Worker's normal API, then revokes it and cleans up. It
does not use the installed App, personal library or a privileged fixture route.
It requires prior authorization for the target public deployment and network
test; it is not part of the default local suite.

```sh
WENLAN_NO_AUTOSTART=1 WENLAN_TEST_PUBLIC_RELAY=1 \
  WENLAN_NATIVE_BIN_DIR=/absolute/path/to/target/debug \
  WENLAN_TEST_FASTEMBED_CACHE=/absolute/path/to/existing/model-cache \
  WENLAN_CLOUDFLARED_BIN=/absolute/path/to/cloudflared \
  WENLAN_CODEX_BIN=/absolute/path/to/codex \
  node --test --test-reporter=spec relay/tests/public-native-check.mjs
```

The native binary directory must include `wenlan-server`, `wenlan-mcp` and
`examples/seed_reviewer_library`. Omitting `WENLAN_CODEX_BIN` runs only the API
lane. With it, an isolated Codex OAuth login and native app-server scan/call
the tools, check five positive and three negative cases, and check denial after
revocation. No model inference is requested, no user login credentials are
copied, and unexpected app-server permission requests are rejected. This is
native client protocol evidence, not ChatGPT/browser UX or a demo recording.

The isolated MCP login uses app-server `mcpServer/oauth/login`, validates its
returned authorization URL, completes only the owned synthetic pairing, and
waits for the matching `mcpServer/oauthLogin/completed` success notification.
It does not invoke `codex mcp login`: that CLI opened unsolicited browser
pairing tabs on macOS despite `BROWSER=/usr/bin/false`. The app-server path
keeps this protocol test headless; real user OAuth UI remains a separate gate.

For the native reverse path, compile the ignored helper in the App test target
and use the executable path reported by Cargo. This is the actual Rust socket
implementation, not a JavaScript WebSocket substitute:

```sh
WENLAN_NO_AUTOSTART=1 cargo test -p wenlan-app --lib --no-run --offline
WENLAN_NO_AUTOSTART=1 WENLAN_TEST_PUBLIC_RELAY=1 \
  WENLAN_PUBLIC_TRANSPORT=reverse \
  WENLAN_REVERSE_HELPER_BIN=/absolute/path/to/compiled/wenlan_lib-test-binary \
  WENLAN_NATIVE_BIN_DIR=/absolute/path/to/target/debug \
  WENLAN_TEST_FASTEMBED_CACHE=/absolute/path/to/existing/model-cache \
  WENLAN_CODEX_BIN=/absolute/path/to/codex \
  node --test relay/tests/public-native-check.mjs
```

Reverse mode never starts cloudflared. It prepares a pending device through
`/devices/reverse`; the helper uses the production relay URL, authenticates its
outbound socket and waits for the real protected connector probes to activate
it. Credentials travel through an owner-only scratch file, not arguments or
logs. The helper catches SIGTERM and shuts down its owned connection. This
mode has passed real Codex OAuth, discovery, five positive and three negative
tool cases, refresh, session restart, revocation and final process cleanup.
By default it does not run model inference or establish the unattended reviewer service.
The reverse lane also observes the real SDK heartbeat over a public GET SSE
stream, aborts that request, and checks that the next authorized tool responds.
This does not measure exact remote task release; no public pending-task metric
or native cancellation acknowledgment is exposed by this test.

For a separately approved model-mediated run, set `WENLAN_TEST_MODEL_EXECUTION=1`
and `WENLAN_CODEX_MODEL_HOME` to a dedicated, already authenticated temporary
Codex home. Create its private parent with `mktemp -d` using the prefix
`wenlan-codex-model-`, then create a private `codex` child and complete the
ordinary official `codex login` there. Use the canonical absolute path. The
parent must be directly under the OS temporary directory or `/tmp`; both
directories must be owned by the current user with no group/other access.
The login file must be a private regular file, and `config.toml` must not exist.
Never point this lane at a personal Codex home or copy existing credentials.

This opt-in uses the same synthetic MCP OAuth login. The default
`WENLAN_CODEX_MODEL_CASES=positive` requests one bounded Luna/low turn covering
five positive tool scenarios. It checks actual
completed tool-call notifications, arguments, projected results and the final
answer, not merely the turn-start response. Other plugins, shell and browser
tools are disabled; unexpected server requests remain denied. The 90-second
model window cannot be combined with an attended portal window. The test removes
only its newly created config and revokes its synthetic relay device; it retains
the dedicated model login for subsequent authorized testing.

`WENLAN_CODEX_MODEL_CASES=negative` instead starts a fresh ephemeral thread and
uses the three exact negative prompts in `chatgpt-app-submission.json`. Each
turn has a 45-second deadline. The lane requires completed, nonempty answers,
the expected general-knowledge answer or unsupported-action explanation, and
no MCP, shell, file-change, dynamic-tool or web-search activity. It retains
captured failure evidence rather than silently accepting different behavior.
Refusal wording assertions are lexical, not a complete semantic evaluator;
inspect the actual answers before accepting a receipt. This lane does not
establish ChatGPT UI results or continuously available reviewer hosting.

### Isolated macOS App lifecycle

`tests/native-app-lifecycle-check.mjs` is a separately approved foreground
native test, not part of the default unit suite. Build the App with
`tauri/custom-protocol` after `pnpm build`, and set build-time `TAURI_CONFIG`
to `{"identifier":"com.wenlan.desktop.dev.relay-review","productName":"Wenlan Relay Review"}`.
Use the same override if bundling. A runtime environment variable alone does
not change the compiled single-instance identity. Never use the installed App.

The probe requires `WENLAN_TEST_NATIVE_APP=1`, absolute `WENLAN_APP_TEST_BIN`,
`WENLAN_REVIEWER_SEED_BIN`, `WENLAN_TEST_FASTEMBED_CACHE`, and the independently
verified `WENLAN_APP_TEST_SHA256`. It creates a new synthetic library, private
HOME/config/log roots and unused daemon port. Two launches must report the
scratch knowledge path and exit cleanly on SIGTERM, recording their App-owned
sidecar cleanup and closed listener. `WENLAN_APP_INSPECTION_MS` optionally holds
the first launch for at most 180000ms for separately authorized UI inspection.
Only this optional inspection window extends the overall test budget; daemon
readiness and graceful-exit deadlines remain unchanged.
Only the spawned App is signalled; no name-based process termination is used.
It preserves scratch logs and a JSON receipt for inspection. This probe does
not enable remote access itself, prove sleep/wake, verify rendered UI by itself, or
provide an always-on reviewer service.

For an approved attended synthetic connection check, additionally set
`WENLAN_APP_TEST_RELAY=1` and `WENLAN_TEST_PUBLIC_RELAY=1`, with a nonzero
`WENLAN_APP_INSPECTION_MS`. This keeps the bounded inspection window open on
both launches. Enable only the seeded `atlas-review` Space through the real App
UI. The harness tracks the private profile in its newly created scratch root,
requires the same enabled device identity across restart, stops the owned App,
and revokes every tracked device in final cleanup, including after failures.
It never samples a personal profile or requires a private reverse-connection ID.
Failed cleanup fails the test and retains scratch for scoped recovery.
These file observations establish persistence, not actual remote connectivity
or client OAuth. Record rendered connection state and actual client queries
separately. `WENLAN_APP_OFFLINE_MS` (integer 0..90000, default 0) optionally
holds the harness between cycle-1 stop and cycle-2 start, after the cycle-1
log is written and daemon-port closure is verified. It logs
`WENLAN_APP_OFFLINE` with only `scratch`/`daemonPort`/`durationMs`, awaits
that bounded interval, then reasserts the daemon port is still closed. Nonzero
requires the same relay/public opt-ins and leaves the persisted profile
untouched; final cleanup still revokes every tracked device. Two 180s UI
windows plus the 90s maximum fit the existing 700s relay budget. This is a
window for real client observation, not proof by itself: record actual client
behavior separately. Local helper coverage runs without network or App launch:

```sh
node --test relay/tests/native-app-relay.test.mjs relay/tests/private-json.test.mjs
```

An attended portal scan can opt in with `WENLAN_PORTAL_PAIRING_ID` and
`WENLAN_PORTAL_CLIENT_ID`, both taken from the current OAuth consent page.
The helper requires the exact client, public MCP resource, `wenlan:query` scope
and the synthetic `atlas-review` Space before approval. Its default 90-second
window emits `WENLAN_PORTAL_READY`; `WENLAN_PORTAL_WINDOW_SECONDS=180` permits
one longer attended ChatGPT check. No other duration is accepted. Complete the
browser callback promptly: at least 35 seconds must remain on the pairing code
when it is approved, but the OAuth tool window may outlive the consumed code.
The normal final revocation and cleanup still run. This grants no personal
library access and is not production auto-consent or a continuously available
reviewer backend. Never use these test values to configure a real user account.

In the default tunnel mode, the Worker verifies anonymous denial and the authenticated connector
contract during enrollment. A client's inability to resolve its own public
tunnel is diagnosed separately with the opt-in `tunnel-dns-check.mjs`; the live
integration does not override DNS or TLS. Quick Tunnels remain a development
transport, so a passing test does not establish production streaming/uptime.

Do not run this lane in a retry loop. The production admission limits include
three `/devices` requests per peer per fixed UTC hour and ten DCR requests.
Honor any `429` and `Retry-After`; do not raise limits, switch peers, or repeat
an uncertain enrollment to obtain a passing test. The normal cloudflared
shutdown grace is 30 seconds; cleanup allows 35 seconds before force-kill.
Any cleanup failure fails the test and retains the private scratch directory
for scoped recovery. A printed passing phase is not an overall test pass.

## Service-name migration

The reviewed gateway is deployed as `wenlan-relay`; the legacy Worker's name
remains unchanged. The separately approved account-subdomain cutover moved
both Worker URLs to `wenlan-app.workers.dev`. Installed legacy clients that
still use `https://origin-relay.originmemory.workers.dev` lose that route and
require an explicit client update. The candidate App uses the new gateway
directly; no installed App was replaced by the hostname cutover.

Verify the new endpoint and update the desktop integration before retiring the
old service. Do not copy the legacy unauthenticated registration/proxy into the
new Worker merely to obtain the new name. The old Worker remains deployed but
is not a fallback for the new gateway. It cannot restore new OAuth grants.
Hostname changes require new-origin discovery and real-client consent checks;
historical tests at the old origin do not establish that the new origin works.
A Worker version rollback does not undo the account-subdomain rename.

Cloudflare does not allow a Worker version rollback across a Durable Object
class lifecycle migration. In particular, after the first `RelayAuthority`
migration, do not assume `wrangler rollback` can restore the pre-migration
compatibility Worker. Before that first deployment, preserve the exact reviewed
source, configuration and bundle, and validate a recovery deployment retaining
the same authority class, namespace and storage contract. Do not delete storage
or forward new authenticated requests through the old unauthenticated service
as an outage workaround. A code rollback does not restore KV or authority data.
See [Cloudflare rollback limits](https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/#bindings).
