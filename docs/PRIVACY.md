# Privacy Policy

Wenlan is a local-first personal memory system. This policy covers everything the project ships: the desktop app, the daemon, the CLI, the MCP server, and the Claude Code and Codex plugins.

## What data Wenlan stores

Only what you explicitly capture: decisions, lessons, observations, project context, and wiki pages synthesized from those memories. Wenlan does not monitor or scrape your machine for material to store.

It does keep a local record of its own use, in two tables of the same database that holds your memories. One table logs actions: searching, storing, refining, and forgetting. Each row names the action, which agent did it, when, which memories were involved, and a short description.

Two of those descriptions carry your own words. A search stores the query you typed. Forgetting a memory stores its title, deliberately, because the memory itself is gone by then and the title could not be looked up later. The other table records which memory was read and when; results that are pages rather than memories are left out of both.

Some of this is cleared as you go: deleting a memory removes its read records, and `wenlan cleanup-legacy-captures` clears both tables. Nothing expires rows with age, so otherwise this history grows for the life of the install. Deleting the database, as described under "Data deletion" below, deletes it too.

One thing is automatic, and you switch it on yourself. When you connect a folder as a source, Wenlan watches it and reads changed files without asking again for each one -- it syncs that folder once at startup and then whenever a file in it changes. Choosing the folder is the consent; each individual file is not a separate prompt.

## Where data is stored

Your data is stored on your machine:

- `~/.wenlan/pages/` -- wiki pages (Markdown)
- `~/.wenlan/sessions/` -- session logs (Markdown)
- `~/.wenlan/sources/` -- uploaded source documents
- `~/.wenlan/spaces.toml` -- Space mapping config
- `~/.wenlan/db/` -- symlink to the platform data directory (macOS: `~/Library/Application Support/wenlan/`; Linux: `~/.local/share/wenlan/`; Windows: `%LOCALAPPDATA%\wenlan\`), with the libSQL database at `<data dir>/memorydb/`
- `~/.wenlan/bin/` -- installed binaries
- The desktop app's log, `wenlan.log`, outside all of the above: `~/Library/Logs/com.wenlan.desktop/` on macOS, `~/.local/share/wenlan/logs/` on Linux, `%LOCALAPPDATA%\wenlan\logs\` on Windows. It is a single file that is never rotated or trimmed. It holds diagnostics rather than your memories. It can still name paths of files it had trouble indexing, so read it before attaching it to a bug report. It does not contain the Remote Access identifier; releases up to and including v0.17.4 wrote the full relay address here, which carried that identifier, so a log kept from one of those versions should be deleted rather than shared.

The daemon listens on `127.0.0.1:7878`, which is your own machine only. Nothing you capture -- no memory, page, source, or search -- is sent anywhere unless you turn on one of the features in "Sending your content somewhere" below.

One exception needs nothing turned on. If a note contains a remote image, displaying that note sends the image's address, which came out of your note, to whoever hosts it. See "Images in your notes reach their host" below.

## When Wenlan reaches the network

Wenlan is not fully offline. Three things happen on their own, and each tells the other end your IP address. The first is a download rather than a single request: it fetches the model file plus its tokenizer and configuration files, so expect several requests. Only one of the three has an off switch, noted in the table.

Separately, displaying a note that contains remote images may reach those image hosts. That is described under "Images in your notes reach their host" below. Two more requests happen only when you press something: installing an optional model, covered in "Sending your content somewhere", and installing the Claude Code or Codex plugin from Settings, which runs that client's own CLI and clones this repository's plugin marketplace from GitHub. One more can happen without you pressing anything, in another program: when no `wenlan-mcp` binary is installed, Wenlan writes an `npx -y wenlan-mcp` command into your AI client's configuration, and that client fetches the package from npm the first time it starts.

| What | When | Where | What it sends |
| --- | --- | --- | --- |
| Search model download | Once, the first time the daemon starts | `https://huggingface.co`, repository `Qdrant/bge-base-en-v1.5-onnx-Q` | Downloads about 210 MB across several files. Sends no memory content. One caveat: the download library reads a Hugging Face access token from that tool's standard cache directory if you already have one saved on this machine, and attaches it, which identifies your Hugging Face account to them. This model is what makes local search work; without it the daemon cannot start. |
| Desktop app update check | About 3 seconds after launch, then every 30 minutes while the app is running, or when you choose Check for updates. Skipped entirely when the app starts with `WENLAN_DATA_DIR` or `ORIGIN_DATA_DIR` set. | `https://github.com/wenlan-app/wenlan/releases/latest/download/latest.json` | Nothing but the request itself, identified as `tauri-plugin-updater`. If a newer version exists the app asks you; it never installs without your click. Choosing Later suppresses automatic reminders for that version for 24 hours, including across restarts. |
| MCP server version check | Every time an AI client starts `wenlan-mcp`, normally at most once a day | `https://github.com/wenlan-app/wenlan/releases/latest` | Nothing but the request itself, identified as `wenlan-mcp/<version>`. It reads only where GitHub redirects, to compare version numbers. The daily limit is remembered in a cache file; where that file cannot be written, such as a read-only home directory, the limit lasts only for that one process, so each new one can check again. |

To avoid the model download, copy an existing model cache into the daemon's cache directory before first start. To avoid the two version checks, block `github.com` for those processes at your firewall; both fail quietly and Wenlan keeps working.

### Images in your notes reach their host

These requests deserve their own heading, because they are the ones that can reach a server nobody at Wenlan chose, and because a note can produce many of them.

Wenlan renders Markdown, and a Markdown image may point at any address. If a note contains `![something](https://example.com/picture.png)`, **displaying that note tries to load the picture from `example.com`**. Every remote image in a note is a separate load, so a note with ten of them names up to ten hosts, and a redirect can pull in further hosts that the note never named. How many network requests that becomes depends on your browser cache and on those redirects. The desktop app sets no content security policy of its own, so it does not block them, though your network or the webview still might.

A load that does reach the host tells it your IP address, the time, and the address it asked for, which came out of your note. Wenlan adds nothing to that address; the path and any query string already in it are sent, while a fragment after a `#` stays in the browser. A uniquely generated image address therefore works as a read receipt on anyone who opens the note.

It may tell the host more than that. The reading view uses an ordinary image tag and sets nothing on it, so your browser's own defaults decide the rest: it may send cookies that host has already set in this app, subject to the usual same-site and scope rules, and it normally sends a `Referer` naming the app's address rather than the note. The editor sets `referrerpolicy="no-referrer"`, which drops the referrer but does not affect cookies. Exactly what is sent is your webview's behaviour, not something Wenlan controls or can promise.

Reading and editing differ. The reading view builds every image as soon as it renders the note. The editor builds images only for the part of the note on screen, and marks them to load lazily, so an image far below your cursor is not fetched until you scroll to it.

This matters most for content you did not write: a page imported from elsewhere, or a document someone sent you. There is no switch to turn it off.

Nor is there a way to point a note at a picture in your own notes folder. A relative address such as `![x](pictures/photo.png)` is passed through unchanged by the reading view, which resolves it against the app rather than your folder, so your file does not appear; the editor accepts only `http:` and `https:` addresses and skips it. The app can display local files through the mechanism it uses for avatars, which is limited to the folders it is configured for and reaches no network -- on Windows that mechanism produces an address beginning `http://asset.localhost/`, which looks remote but is not. Outside those folders, an image you can see in a note came over the network.

If a note worries you, remove the image link from it.

## Sending your content somewhere

These are off until you turn them on. Aside from the remote images described above, which need nothing turned on, they are the ways your captured content leaves your machine that we know of.

- **A cloud AI provider (bring your own key).** If you save an API key and pick that provider for enrichment, Wenlan sends the text of the memory or document being processed, plus its prompt, to that provider. Anthropic goes to `https://api.anthropic.com/v1/messages`. Any other provider goes to the endpoint you entered -- the app offers presets for OpenAI, Google, Groq, OpenRouter, Mistral, DeepSeek and xAI, and an endpoint on your own machine such as Ollama stays local. What that provider does with the text is governed by its own terms. Read them rather than assuming: several of these companies say their consumer privacy policy does not cover text submitted through their API, and point to a separate business or API agreement instead. Your key is stored in the local config and sent only to that provider. Turn it off with `wenlan steep disable`, or by clearing the key.
- **Testing a provider.** The "test endpoint" button sends one fixed sentence, `Say 'hello' and nothing else.`, and the model-list button asks the provider what models it offers. Neither sends anything you captured.
- **Web access (experimental, desktop app only; called Remote Access in earlier versions).** When you turn it on, the app connects out to Wenlan's web service at `https://relay.wenlan.app` so web and phone AI apps, such as Claude and ChatGPT, can search what you choose to share: the whole library (every Space, plus everything not in a Space) or one Space. When you allow an app you also choose what it can do: search only (the default), or search and write, which lets it add, edit and delete memories and pages in what you share. No tunnel program runs. The service keeps the records listed under "Pre-release standalone `wenlan-relay` connector" below: a device key that ends after 90 days unless you renew it, what you chose to share (one Space, or a marker meaning the whole library), and a record of each app you allow. An app is allowed only after you click Allow in a Wenlan window that shows which app is asking; you can decline, and you can remove an app at any time under Connected apps. While access is on, a search from an allowed app travels through the service to your computer and the result returns the same way, so the service and the AI app can read what is searched and found. The same holds for what a write app saves, edits or deletes: the text it sends passes through the service. Writes are marked so that AI apps can ask you before making them; whether and how an app asks is up to that app. Delete is marked as destructive. Turning Web access off, or renewing the key, ends every connected app right away. Nothing is sent while it is off.
- **On-device model download.** If you run `wenlan models install` or start the download from Settings, a Qwen model is fetched from `https://huggingface.co`. This is separate from the search model above and does not happen on its own. Once installed, enrichment runs on your machine and the text being enriched does not leave it.
- **Better search ranking.** If you turn on the reranker, its weights are downloaded from `https://huggingface.co` the next time the daemon starts, between roughly 146 MB and 1.1 GB depending on which one you choose. It is off unless you set it.

### Connected AI clients and the query-only MCP profile

When you connect an AI client to Wenlan, that client receives the results of
the tools it calls. A client operated by a cloud provider may send those
results to that provider as part of your conversation. Keeping the database
local does not keep retrieved content out of the connected AI client. The
client provider's own terms and data controls govern its handling of that copy.

The opt-in `wenlan-mcp serve --tool-profile query-only` profile exposes only
`brief`, `recall`, and `get_page_sources`. It receives the query or topic,
Space selection, search filters, or page identifier needed for the lookup.
It returns matching knowledge, a project Brief, or supporting source content.
Source identifiers support citations and follow-up retrieval; archive and
pending-review flags provide context for interpreting results.

This profile removes raw ingestion text and unnecessary ranking, hashing,
agent-attribution, timing and access metadata from successful tool responses.
It does not redact personal information embedded in titles, knowledge content
or Brief text. Choose carefully what library you connect and what you ask the
client to retrieve. The profile is not a sensitive-content classifier.

Query-only limits knowledge operations, not all side effects: `recall` records
the query and accessed memory identifiers in the local activity history
described above. That history does not expire by age. The profile does not
expose tools to save, edit or delete knowledge, so local deletion controls
remain separate from the query connector. Disconnecting a client does not
delete content that the client or its provider has already received.

The profile requires a non-empty bearer token, but this is not OAuth,
per-user identity, or multi-user/Space authorization. It does not modify the
experimental desktop Remote Access flow described above. This source-level
profile is not a claim that a public ChatGPT or Codex service is deployed or
approved. A hosted offering needs its actual operator, recipients, retention,
authorization and revocation controls documented before public release; do not
assume this local profile supplies those controls.

### Pre-release standalone `wenlan-relay` connector

**Status.** This is a pre-release technical disclosure, not legal or security
approval. The standalone relay source is now in this repository and its
candidate public origin is `https://relay.wenlan.app`.
It is separate from the legacy released Remote Access flow, which uses
`https://origin-relay.originmemory.workers.dev/register` and remains described
above. No automatic migration of legacy records is claimed.

The local database stays on your machine. The standalone relay is not designed
to replicate your knowledge library: its durable records are a control plane for
authentication, devices, routes, grants and sessions. When you use it, the
query request travels through the Cloudflare Worker to the connected local
connector and the result returns through Cloudflare to the connected AI client.
That client, and its provider where applicable, can receive the query and
returned knowledge as part of the conversation.

The public MCP endpoint is `https://relay.wenlan.app/mcp`. OAuth grants access
only to what was approved during pairing: one existing Space, or the whole
library. A one-Space grant does not include new Spaces. A whole-library grant
reaches every Space, including Spaces created later, plus everything not in a
Space. Normal desktop pairing does not require a separate
Wenlan email/password account. A search-only grant (scope `wenlan:query`)
exposes `brief`, `recall`, and `get_page_sources`. A grant the user approved
for writing also carries scope `wenlan:write` and additionally exposes
`read_page`, `list_pages`, `capture`, `write_page`, `rename_page`, `forget` and
`delete_page`, served on the device by a separate `web-write` MCP process that
runs only after the user has approved writing for at least one app. The local
query activity described above still applies.

Cloudflare operates the relay and its storage, and Wenlan's operator administers
the service. Requests and results are readable while the relay processes them:
HTTPS is not end-to-end encryption that excludes either operator. The service
is not designed to persist tool request/result bodies in KV or Durable Object
records, but that is not a guarantee that infrastructure providers retain no
diagnostics. Management credentials are hashed in relay state; backend
credentials remain usable so the relay can authenticate to the connector.
Not all credentials are stored as one-way hashes.

The relay's control-plane records include:

- Device and route state: generated device/subject identifiers, a hash of the
  management credential, enabled/revision/expiry state, the shared Space or whole-library marker, tunnel
  origin or opaque reverse-connection identifier, and backend connector
  credential needed to route the request. Pending reverse enrollment retains
  the proposed Space and backend credential without enabling an OAuth route.
- Pairing and authorization state: a hash of the browser secret, pairing and
  authorization identifiers, client ID, resource, requested scope, whether the
  user allowed search only or writing, status, expiry,
  device/route generation and explicit Space approval. The stored validated
  OAuth request includes its redirect/resource/client fields and S256 PKCE
  challenge and method.
- OAuth client and grant state: provider-managed client registration metadata,
  token records, and grant receipts containing client/device/route/Space
  bindings, authorization IDs, status, timestamps, expiry and cleanup state.
- Session and abuse-control state: opaque public-to-backend MCP session
  mappings, backend session IDs, binding hashes, status and expiry, plus
  request rate-window counters and expiry keyed by a hash of the connecting
  IP, of the device identifier, or of the OAuth grant.
- Request log lines in Cloudflare Workers Logs: for each request, its route
  (with identifiers replaced by `:id`), method, status, duration, whether it
  presented an app or device credential, which limit refused it, the name of
  an allowlisted AI app or `other`, a 12-character hash of the device
  identifier, and the error type of an internal failure. They contain no IP
  address, token, Space name, request or response body, query or result.
  Cloudflare keeps them for its Workers Logs retention period.

Technical expiry denies access; it is not a physical deletion time. Pairing and
authorization requests and pending reverse enrollments expire after 5 minutes,
route and session mappings after
24 hours, management credentials after 90 days, access tokens after 15 minutes,
refresh tokens after 30 days without use (each use renews them, up to the
consent limit), client registrations after 90 days from registration (a
registration used in its second half renews), and consent/grant records after
90 days. Device revocation
disables the device and route. Grant revocation records denial before asking
the OAuth provider to delete its token records; failed cleanup remains pending
and is retried by bounded background maintenance: scans are paged, at most one
provider cleanup batch runs per alarm, alarms recur every 60 seconds while work
remains, and failed retries back off up to one hour. The source establishes no
physical-deletion SLA. Revoking relay access does not delete a query or result
already received by the connected AI client or its provider.

An optional reviewer account uses a separate synthetic library and the same
OAuth authorization path. Its operator configuration contains a username,
password hash, device and Space binding, and expiry. Login expires no later
than thirty days or the device credential, whichever comes first. Expiry
disables new logins; the operator must separately remove expired configuration
and dispose of the synthetic library. This does not create a general user
account or transfer ownership of a device.

To stop access, disable Remote Access or revoke the relevant client grant.
Changing the connected Space requires fresh consent. If the app reports a
pending disconnect, remote revocation has not been confirmed and must be
retried. After a confirmed disconnect, the old device-management credential is
removed from the local profile and the local MCP bearer is replaced; settings,
knowledge and activity records remain until separately deleted. Previously
shared copies and backups require their own deletion controls. Do not connect
a Space containing credentials or other restricted information: the connector
does not classify or redact sensitive text embedded in knowledge.

Abuse-control counters use fixed calendar windows: per-IP-hash, per-device and
per-grant counters expire at the end of the current minute or hour, depending
on the endpoint; aggregate
daily enrollment and client-registration counters expire at the end of the UTC
day. The application stores a hash rather than the raw connecting IP in these
counters. Expired rows are removed in batches of at most 256 by background
maintenance, which schedules its next alarm after 60 seconds, or when storage
capacity requires cleanup. An outage or maintenance backlog can delay physical
deletion beyond expiry. Cloudflare receives the network connection itself;
hashing the application's counter key does not hide your IP from Cloudflare.

## Telemetry

Optional usage statistics are **off by default**, including when you upgrade an
existing installation. You can choose to enable them in Settings. This choice is
separate from connecting an AI model, Remote Access, or subscribing to website
emails. Debug builds and the `WENLAN_TELEMETRY_DISABLED=1` environment setting do
not send usage statistics.

When enabled, the daemon sends small batches of operation counts to
`https://wenlan.app/api/app-events`: daemon readiness, successful saves, searches
with or without results, Wiki generation, and fixed operation-error categories
where those operations are instrumented. Batches include the Wenlan version and
platform (macOS, Windows, Linux, or other), not an installation or account ID.
These are operation counts, not a record of individual users or a claim that a
search result was helpful. The receiver records them under its UTC receipt day.
An accepted save may include a duplicate or a pending revision, not a new memory.
Coverage is limited to the instrumented daemon save, search/Brief and page
creation/distillation request handlers; direct source-sync writes and background
refinery/scheduler page generation are not counted by this version.

No note, query, prompt, answer, title, file path, memory/entity/source identifier,
email, relay identity, content hash, free-text error, or crash trace is included.
The existing local activity history and onboarding content previews are never
uploaded by this feature. Wenlan does not install a crash-reporting SDK. The
app's fonts remain bundled rather than fetched.

The local consent preference is persisted separately from your knowledge.
Unsent counts are bounded and kept in memory, not a durable activity log. They
are batched at most hourly and can be lost on exit or network failure. There are no retries of an attempted
batch and no upload of pre-consent history. Turning the setting off discards
unsent counts and stops future batches. Cancellation is best effort: a batch
already in flight may still arrive after you turn it off and cannot be retracted.
CLI and MCP operations are counted at daemon boundaries
too, so using the GUI is not a prerequisite.

Vercel receives the HTTPS request and the dedicated Supabase database stores
aggregate counts. Our application tables do not store your IP address or raw
HTTP headers, but network providers necessarily process connection metadata.
This is not a promise of anonymity against those providers. Aggregate records
have no automatic expiry in this version. Because there is no installation or
account identifier in the aggregate, we cannot retrieve or delete one person's
past contribution separately. We do not join these counts to website visits,
email subscriptions, Remote Access accounts, or GitHub download counters.

One caveat about the dependency list, since it is public and you may read it. A development-only profiling tool, React Scan, pulls crash-reporting packages into the lock file. It is loaded only in a development build, behind two environment flags, and is not part of a release.

The local activity history described under "What data Wenlan stores" stays in
the database on your machine, regardless of this optional statistics setting.
The desktop app also keeps the diagnostic log described above. The standalone
relay separately keeps the
authorization, routing, session and abuse-control records described above.
Its deployment configuration keeps only the request log lines described above
in Workers Logs and turns Cloudflare's own invocation logs off; this is not a
promise that Cloudflare retains no service-level data.

Opening a window starts the update check listed in the table above about three seconds later. On a first run it also triggers the search model download in that table, and if you left Web access on, it connects to Wenlan's web service again. Beyond those, see "Images in your notes reach their host" above, because the note you open can reach the network on its own.

## The other companies involved

Wenlan reaches these services, so their own terms decide what they do with the request.
The project can see Cloudflare relay traffic figures and, when optional usage
statistics are enabled, the aggregate counters and hosting diagnostics described
above. The standalone relay operator can also access the persisted control-plane
records described above, but it does not store a replicated knowledge library
by design. Wenlan's relay runs on Cloudflare Workers, so the project holds a
Cloudflare account and is bound by Cloudflare's own terms as a customer.

| Service | Why Wenlan reaches it | Their policy |
| --- | --- | --- |
| Vercel | Receives optional usage-statistics batches at wenlan.app | [Vercel Privacy Policy](https://vercel.com/legal/privacy-policy) |
| Supabase | Stores the optional aggregate operation counts behind the website receiver; the App never receives database credentials | [Supabase Privacy Policy](https://supabase.com/privacy) |
| GitHub | The two version checks, downloading a release, and installing the Claude Code or Codex plugin from Settings, which clones this repository's plugin marketplace | [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement) |
| Hugging Face | Downloading the search model, and any optional model you install | [Hugging Face Privacy Policy](https://huggingface.co/privacy) |
| npm, which GitHub operates | Only when no `wenlan-mcp` binary is installed, in which case your AI client runs `npx` to fetch it | [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement) |
| Cloudflare | The legacy Remote Access tunnel, or the pre-release standalone connector at `https://relay.wenlan.app` | [Cloudflare Privacy Policy](https://www.cloudflare.com/privacypolicy/) |
| The host of a remote image in one of your notes | Displaying a note whose Markdown points at a remote image | Whoever runs that host. Wenlan cannot know who that is |

### The cloud AI providers Wenlan offers

None of these is contacted unless you save a key and choose that provider for enrichment. Wenlan ships a preset for each one, so each one's published privacy policy is named here.

**Read one thing before relying on the table.** Wenlan talks to these providers through their APIs, and several of them state in the very policies below that the policy does not cover content submitted through an API by a business or developer customer. Anthropic, OpenAI and Groq each say a version of this and point to a separate customer or API agreement instead; xAI states the exclusion without naming the document that replaces it. Whichever agreement applies is what actually governs the text Wenlan sends, and it was accepted by whoever created the key -- you, or the organization that issued it to you. Find it on the same provider's legal page; we deliberately do not guess which version applies to your account.

| Provider | Their policy |
| --- | --- |
| Anthropic | [Anthropic Privacy Policy](https://www.anthropic.com/legal/privacy) |
| OpenAI | Published per region: [rest of world](https://openai.com/policies/row-privacy-policy/), [United States](https://openai.com/policies/us-privacy-policy/), [Europe](https://openai.com/policies/eu-privacy-policy/) |
| Google, for Gemini | [Google Privacy Policy](https://policies.google.com/privacy) |
| Groq | [Groq Privacy Policy](https://groq.com/privacy-policy) |
| OpenRouter | [OpenRouter Privacy Policy](https://openrouter.ai/privacy) |
| Mistral | [Mistral Privacy Policy](https://legal.mistral.ai/terms/privacy-policy) |
| DeepSeek | [DeepSeek Privacy Policy](https://cdn.deepseek.com/policies/en-US/deepseek-privacy-policy.html) |
| xAI | [xAI Privacy Policy](https://x.ai/legal/privacy-policy) |

Wenlan also offers presets for Ollama and LM Studio. Wenlan sends those requests to an address on your own machine, so nothing you capture leaves it by that route; what those two programs do on their own is up to them, not something this project can promise. There is also a custom option where you type the address yourself. For a custom address, the policy is whatever the operator of that address publishes.

Wenlan's legacy relay, at `origin-relay.originmemory.workers.dev`, is run by this project rather than a third party. It is meant to hold the tunnel address you register and the random identifier described above, and nothing more. Its source is not part of this repository, so that is a statement of intent you cannot check against the code here. It runs on Cloudflare Workers, so Cloudflare sees the requests as the platform underneath it. The pre-release standalone `wenlan-relay` connector is described above and is a separate source-backed service.

## Data deletion

- Delete individual memories: `/wenlan forget <id>` in the plugin.
- Delete everything: remove `~/.wenlan/` and your platform data directory (`~/Library/Application Support/wenlan/` on macOS, `~/.local/share/wenlan/` on Linux, `%LOCALAPPDATA%\wenlan\` on Windows). An install upgraded from Origin still holds a full copy of its data in `~/.origin/` and in the sibling `origin` data folder (`~/Library/Application Support/origin/` on macOS, `~/.local/share/origin/` on Linux, `%LOCALAPPDATA%\origin\` on Windows); delete or copy those two as well. Three things sit outside all of those:

  - `~/.config/wenlan-mcp/` holds the Remote Access identifier, which doubles as that feature's shared secret, and the MCP bearer token if you generated one. Neither is removed with the folders above.
  - If you pointed the knowledge or page path at a folder of your own, your pages are in that folder, not under `~/.wenlan/`.
  - Registering a legacy Remote Access tunnel leaves a record on the legacy relay. Turning legacy Remote Access off stops the tunnel on your machine; the app sends no request to remove that legacy registration. Open an issue if you want it deleted. The standalone `wenlan-relay` connector has separate device and grant revocation and asynchronous cleanup described under "Pre-release standalone `wenlan-relay` connector"; technical expiry is not a physical-deletion SLA.
  - The log file listed under "Where data is stored" is in none of those folders and is not removed with them.
  - Wenlan writes an entry for itself into the configuration of each AI client you connect -- Claude Desktop, Claude Code, Cursor, Gemini, Codex, Muse Code. Those entries stay after uninstall, and the client will keep trying to launch Wenlan. Remove them by hand. The command-line tool also leaves a timestamped backup of each file it edited, beside the original.
  - An install upgraded from Origin may also have `~/.config/origin-mcp/`, holding that version's identifier and token.
- Uninstall the daemon: run `wenlan background off` to stop it and disable autostart (this is a reversible runtime stop, not an uninstall), then remove the service registration for your platform -- `~/Library/LaunchAgents/com.wenlan.server.plist` (macOS), `~/.config/systemd/user/wenlan-server.service` (Linux), or the `WenlanServer` scheduled task (Windows) -- and delete `~/.wenlan/bin/`.
- **Uninstall the desktop app:** turn off *Run Wenlan in background at login* in Settings and quit the app (this removes `~/Library/LaunchAgents/com.wenlan.server.plist` and `com.wenlan.desktop.plist` on macOS), then delete `Wenlan.app`; on Windows run the uninstaller from Apps & features. The data folders above stay until you delete them.

## Contact

Questions or concerns: open an issue at https://github.com/wenlan-app/wenlan/issues.

GitHub issues are public. Do not post memory content, access tokens, Remote
Access identifiers or unredacted logs there. Ask for a private contact method
before sharing information needed for an individual data request.

Last updated: 2026-09-09.
