<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-mobile.png">
    <img src="./docs/assets/readme-banner.png" alt="Wenlan: A living personal wiki. AI organizes. You stay in control." width="100%">
  </picture>
</p>

Wenlan turns your documents, Markdown files, and AI conversations into editable notes with links to their sources, so you and your AI tools can keep building on them.

As sources change, you can ask AI to update the notes or enable background updates. If you’ve edited a note, Wenlan proposes revisions for you to review instead of automatically overwriting your work.

<p align="center">
  English | <a href="./README.zh-Hans.md">简体中文</a> | <a href="./README.zh-Hant.md">繁體中文</a> | <a href="./README.es-ES.md">Español</a>
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?sort=semver&label=release"></a>
  <a href="#license"><img alt="License: Apache-2.0 and AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<p align="center">
  <a href="#start-in-30-seconds">Get&nbsp;started</a> ·
  <a href="#what-does-wenlan-build">What&nbsp;is&nbsp;this?</a> ·
  <a href="#what-can-it-do">Capabilities</a> ·
  <a href="#how-does-it-work">Daily&nbsp;workflow</a> ·
  <a href="#evaluation">Evaluation</a> ·
  <a href="#learn-more">Learn&nbsp;more</a>
</p>

https://github.com/user-attachments/assets/35f06749-00e5-484d-a9f4-5e462de8d11e

<p align="center">
  <sub>The opening frame is a composed preview, not a native three-pane view. The rest shows the app in use, including notes and source citations.</sub>
</p>

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>

## Get started

<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>

### 1. Download and open Wenlan

[Download the app](https://github.com/wenlan-app/wenlan/releases/latest), then open it:

- **macOS (Apple Silicon):** open the `.dmg` and drag Wenlan to Applications. The app is signed and notarized.
- **Windows x64:** run the `-setup.exe`. It is not signed yet. If SmartScreen warns you, confirm you downloaded it from the official Releases page before choosing "More info" → "Run anyway".
- **Linux:** no desktop build yet; follow the [setup guide](docs/setup-with-ai.md#install-the-runtime) to use Wenlan with your AI tools without the app.

First launch downloads a model for local search, so keep an internet connection until setup finishes. [Download and privacy details](docs/PRIVACY.md#when-wenlan-reaches-the-network).

<a id="claude-code-in-30-seconds"></a>

<a id="codex-plugin"></a>

<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

### 2. Start with your own note

Choose **Get started** to enter Wenlan, then write a note of your own. The welcome screen also offers a sample to explore first. You can start writing without importing material, connecting an AI tool, or setting up an AI model.

### 3. Bring in sources when you want

You can [import Markdown files or ChatGPT / Claude conversation exports](#what-can-i-bring-in) whenever you are ready. Import is optional; you can begin with a note of your own.

### 4. Connect AI tools when you want AI help

Connect Claude Code, Codex, or other supported AI tools when you want them to use the same knowledge. A connected AI can write notes without an additional local language model.

<details>
<summary>Need help connecting? Ask your AI</summary>

Restart your AI tool if prompted. For help with setup, paste this into Claude Code, Codex, or another tool that can follow a setup guide:

```text
Connect this AI tool to Wenlan by following:
https://raw.githubusercontent.com/wenlan-app/wenlan/main/docs/setup-with-ai.md

Reuse my existing Wenlan installation if available.
Set up only this AI tool, then check that it can save and find a test memory.
```

</details>

To organize a topic with a connected AI, open a work conversation worth keeping and ask:

> Turn the conclusions, reasoning, and sources about [topic] in this conversation into a Wenlan note with source citations.

You can also ask your AI to organize a topic from material you imported into a note.

When it's ready, open the new note in Wenlan, click a citation to check its source, and add your own thoughts. Next time you work on that topic, ask your AI to read this note before continuing.

Background organization and automatic updates are optional and need a [configured model](#models-and-privacy). For terminal installation, other platforms, and updates, see the [setup guide](docs/setup-and-data.md#installation).

Need help? [Setup guide](docs/setup-with-ai.md) · [Report a problem](https://github.com/wenlan-app/wenlan/issues). Issues are public: redact secrets, credentials, access tokens, identifiers, private or personal content from anything you share, including logs.


<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>

<a id="what-is-this"></a>

## Your Wiki, for you and your AI

- **Turn scattered material into connected topics.** Bring documents, Markdown files, and AI conversations together in linked notes, with citations you can open and check.
- **Read it yourself. Use it with your AI.** Read and edit your notes, or ask Claude Code or Codex to use them for your next task.
- **Maintain knowledge without starting over.** Track source changes, update related notes, and keep their history. AI proposes revisions to notes you've edited for you to review.

Your notes are local Markdown files you can read, edit, and take with you.

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-system-mobile.png">
    <img src="./docs/assets/wenlan-system.png" alt="Sources and memories feed an illustrative Agent Loop note. It combines retry guidance with a concrete lesson: tests passed, but a line still covered a mobile label. Desktop and mobile visual checks become part of the completion criteria for later work with Claude or Codex." width="100%">
  </picture>
</p>

<a id="what-wenlan-is-not"></a>

The illustration shows a reusable working rule, not customer results or an automated note-generation test. [How sources, memories, and notes work together](docs/knowledge-guide.md#sources-and-pages).

<a id="knowledge-graph"></a>

<a id="follow-the-connections-behind-a-page"></a>

### Follow the connections behind a note

The Agent Loop example connects a retry rule, a lesson from a UI review, and an acceptance checklist you can reuse.

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-knowledge-network-mobile.png">
    <img src="./docs/assets/wenlan-knowledge-network.png" alt="Illustrative Agent Loop note citing an agent-design guide and a UI review; named memories include retry guidance and a visual-check lesson, linked to a reusable acceptance checklist and related concepts and tools." width="100%">
  </picture>
</p>

<a id="retrieval"></a>
<a id="retrieval-across-words-meaning-and-connections"></a>

Follow a note to related concepts and evidence. [How graph and search work](docs/knowledge-guide.md#graph-and-search).

<a id="what-makes-wenlan-distinct"></a>
<a id="why-is-wenlan-different"></a>
<a id="two-lifecycles"></a>
<a id="two-lifecycles-one-maintained-knowledge-system"></a>

### Knowledge changes. History stays.

New experience improves your notes without hiding earlier decisions. This example replaces “tests passed, so the UI is done” with “check tests and the desktop and mobile views,” keeping the reason and evidence for the change.

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-lifecycle-mobile.png">
    <img src="./docs/assets/wenlan-lifecycle.png" alt="Illustrative before and after: “tests passed, so the UI is done” becomes “check tests and desktop and mobile views”; the evidence is a mobile label obscured despite passing tests. During automatic refresh, changes to a note you edited are proposed for your review." width="100%">
  </picture>
</p>

<a id="local-markdown"></a>
<a id="atomic-memory"></a>
<a id="maintained-page"></a>
<a id="local-markdown-that-works-with-obsidian"></a>

Review applies to AI updates; direct file edits and forced rebuilds follow different rules. [Updates, review, and local files](docs/knowledge-guide.md#updates-and-history).

### Already using Obsidian with AI plugins?

You can use [Copilot](https://docs.obsidiancopilot.com/agent-mode-and-tools/) for AI-assisted reading and editing, [Smart Connections](https://github.com/brianpetro/obsidian-smart-connections) to find related notes, and [Obsidian Git](https://github.com/Vinzent03/obsidian-git) to keep version history. That gives you flexibility, but choosing how the plugins work together, when to organize new material, and how to update older notes is still a workflow you arrange.

Wenlan connects organizing topic notes, tracking source changes, updating notes, and keeping revision history in one workflow, saving you the work of assembling and maintaining those steps yourself. You can read the resulting knowledge directly or let Claude Code, Codex, and other tools build on it.

Ask your connected AI to update a note, or [configure a model](#models-and-privacy) to enable background updates. When AI updates a note you've edited, Wenlan proposes a revision for you to review.

Connect your existing Obsidian vault as a read-only source without moving your files. Wenlan does not modify the vault or sync changes back to it.


<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="what-can-i-bring-in"></a>

## Capabilities

### Bring your material in

- **Keep useful AI conversations:** Import ChatGPT or Claude export ZIPs without duplicating conversations already imported.
- **Bring your existing files:** Import Markdown, text files, or text-extractable PDFs, by file or folder; connect an Obsidian vault as a source. Scanned PDFs need text extraction first.
- **Quick Capture:** Save a thought or decision directly in the desktop app without opening an AI chat.
- **[Capture decisions with your AI](https://wenlan.app/learn/ai-work-memory):** Ask your AI tool to save decisions, lessons, corrections, preferences, and facts, with their sources and what they replace.
- **Bring in another wiki:** Import an external OKF wiki through the CLI or API, preserving source references and links. Reimporting Wenlan's own OKF exports is not supported.

### Explore your personal Wiki

- **[Editable, source-linked Notes](https://wenlan.app/docs/source-backed-pages):** Turn related documents and Memories into Markdown Notes with citations and links to other Notes.
- **Cards or lists:** Browse Notes, Entities, and Spaces in the view that suits you.
- **[Knowledge graph](docs/technical-foundations.md#graph-data-and-entity-resolution):** Explore connections between people, projects, claims, and the Memories supporting them.

### Use it with your AI tools

- **[Pick up in another AI tool](https://wenlan.app/docs/architecture):** Connected Claude Code, Codex, and other MCP clients can use the same local Notes and Memories as the desktop app and CLI.
- **[Search by words and meaning](docs/technical-foundations.md#retrieval-pipeline):** Combine exact matches with local semantic search; graph connections can add relevant context.
- **[Optional deeper search](docs/technical-foundations.md#optional-channels-and-defaults):** Search Notes and finer-grained Memories, with optional reranking to refine results.
- **[Keep projects separate](https://wenlan.app/docs/spaces):** Use Spaces to choose which work, personal, client, or repository knowledge your AI searches.
- **Web access (experimental):** Connect a web AI client such as Claude, ChatGPT, or Muse to your whole library, or to one Space you choose, while your computer is online. Turn it on in Settings with the Web access switch. Queries and results pass through a relay; see [how access works and its privacy limits](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector).
- **Connect your own tools:** Send prepared text, webpage content, or Memories through the local HTTP API. It accepts content, not URLs to fetch.

### Keep knowledge current

Optional background organization and Note updates need a [configured model](#models-and-privacy).

- **Incremental sync:** File and folder Sources track changes in the background. Obsidian vaults stay read-only and resync on demand.
- **[Organize saved knowledge](docs/technical-foundations.md#typed-memory-schema):** A configured model can add types, structured details, relevant dates, tags, search cues, and graph links to Memories.
- **Updates backed by sources:** Automatic refresh rejects drafts with insufficient citations. AI-generated Notes can update; changes to Notes you have edited become review proposals.
- **[Review where judgment matters](https://wenlan.app/docs/review-and-trust):** Review protected conflicts, Note revisions, entity merges, and new vocabulary.
- **See what is happening:** Check progress and blockers in Activity. Configured sync, Steep (background enrichment), and eligible Note updates can continue after the app window closes while the local service runs.

### Keep ownership and control

- **[Local, inspectable knowledge](https://wenlan.app/learn/local-first-ai-memory):** Keep Markdown Notes, citations, revisions, and Obsidian exports; Memories and graph data live in local libSQL.
- **Take your Wiki with you:** Export eligible Notes across Spaces as an OKF v0.2 wiki from Settings or the CLI. This is a wiki export, not a full database backup.
- **[Model choice](docs/technical-foundations.md#model-roles):** Base retrieval stays local. Steep, the optional background enrichment and synthesis, can use on-device Qwen, a local endpoint, or a cloud model; remote providers receive the content needed for their tasks.
- **Health checks and reviewed repairs:** [Doctor](https://wenlan.app/docs/diagnostics-and-issue-reports) and [lint](plugin/skills/lint/SKILL.md) report problems without rewriting knowledge. For supported findings, the app lets you preview and explicitly apply a repair, then verify it.

**Start with a note you want to keep.** [Get started](#start-in-30-seconds). Star this repo if you'd like to come back and try it later.


<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

## Daily workflow

Once your AI tool is connected, you can ask in plain language:

With the Wenlan plugin installed, you can also use the slash commands below.

### Before starting a task

> Find what I've saved in Wenlan about [topic], including earlier decisions and their sources.

Shortcut: `/recall [topic]`

### When you reach a useful conclusion

> Save this decision and why we made it in Wenlan, with its sources.

Shortcut: `/capture [decision, reasons, and sources]`

<a id="when-a-topic-deserves-a-page"></a>

### When a topic deserves a note

> Create or update a Wenlan note on [topic] from the material we've saved. Keep the citations.

To organize saved knowledge into notes: `/distill`

Open the note in Wenlan to read, edit, and check its sources. Next time, ask your AI to use it for your next task.

### When you wrap up

> Save this session's progress, decisions, and open questions in Wenlan so we can pick up next time.

Shortcut: `/handoff`

<details>
<summary>Plugin commands and maintenance</summary>

- **Read the project summary:** `/brief [topic]` reads the current Space's project summary; an optional topic adds related context from that Space.
- **Check and review:** `/lint` checks knowledge health; `/curate` reviews pending captures or revisions.

These shortcuts are available through the Wenlan plugins. Other connected clients use the equivalent MCP tools. Optional background organization and Note updates need a [configured model](#models-and-privacy).

[Full command reference](plugin/skills/README.md).

</details>

<a id="offline-queue-outbox"></a>

[CLI offline writes and replay](docs/setup-and-data.md#offline-queue).

<a id="models-and-privacy"></a>

### Models and privacy

Use your connected AI to write notes without installing an additional local language model. Background organization and automatic updates are optional and need a configured model.

- **Search stays local.** The search model downloads at first launch and runs on your machine, without an API key.
- **Connected AI can receive your knowledge.** A cloud AI client can send retrieved content to its provider. Choosing a cloud model for background organization also sends the material needed for that task. Local storage does not make those interactions local.
- **Usage statistics are off by default.** If you opt in, Wenlan sends limited operation counts, version, and platform, not your knowledge content or an installation ID. [Details](docs/PRIVACY.md#telemetry).

See [network and privacy details](docs/PRIVACY.md#when-wenlan-reaches-the-network) for downloads, update checks, remote images, and optional remote access.

[Model choices and configuration](docs/setup-and-data.md#models).

### Your data and uninstall

Your notes and session logs are Markdown files; memories and the graph live in a local database. You can keep your knowledge when you uninstall the app.

[File locations, backup, and removal](docs/setup-and-data.md#backup-and-removal).


<a id="evaluation"></a>

## Evaluation

This is a retrieval-only snapshot, not a claim about end-to-end answer quality. Method, environment receipts, and the update workflow live in [docs/eval](docs/eval/README.md).

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->


<a id="learn-more"></a>

## Learn more

More detailed documentation, concepts, and comparisons:

### Docs

- [Get started](https://wenlan.app/docs/get-started): install and verify the first local loop.
- [Daily workflow](https://wenlan.app/docs/daily-workflow): brief, capture, recall, handoff, distill, lint, and curate.
- [MCP clients](https://wenlan.app/docs/mcp-clients): connect Claude Code, Codex, Cursor, Claude Desktop, Muse Code, and other clients.

### Workflow guides

- [Build your first LLM Wiki](https://wenlan.app/learn/distilled-wiki-pages-ai-memory): work through a three-source example, check its citations, and review what changes when a source is updated.
- [Build a client project knowledge base for consulting](https://wenlan.app/learn/build-client-project-knowledge-base-for-consulting)
- [Build an investment research knowledge base](https://wenlan.app/learn/build-investment-research-knowledge-base)
- [Build a product research knowledge base before writing a PRD](https://wenlan.app/learn/build-product-research-knowledge-base-for-prd)
- [Build an SRE incident knowledge base](https://wenlan.app/learn/build-sre-incident-knowledge-base)
- [Build a business metric definition knowledge base](https://wenlan.app/learn/build-business-metric-definition-knowledge-base): turn approved KPI specifications into a source-backed data dictionary with formula text, grain, exclusions, owners, revisions, and review state.

### Concepts

- [Why a living wiki, not just AI memory](https://wenlan.app/learn/ai-work-memory): the problem and product model in depth.
- [MCP memory server](https://wenlan.app/learn/mcp-memory-server): how Wenlan exposes knowledge across AI tools.
- [Local-first AI memory](https://wenlan.app/learn/local-first-ai-memory): data, privacy, and control.
- [Markdown and local index](https://wenlan.app/learn/local-first-ai-memory): storage, retrieval, and ownership.
- [AI agent handoff loop](https://wenlan.app/learn/ai-agent-handoff-loop): carrying work cleanly into the next session.
- [Research knowledge base from papers](https://wenlan.app/learn/source-backed-research-knowledge-base): build an inspectable literature matrix and source-backed synthesis from papers you already have.

### Comparisons

- [Wenlan vs Basic Memory](https://wenlan.app/learn/wenlan-vs-basic-memory)
- [Wenlan vs claude-mem](https://wenlan.app/learn/wenlan-vs-claude-mem)
- [Wenlan vs Superlocal Memory](https://wenlan.app/learn/wenlan-vs-superlocal-memory)


## Contributing

Bug fixes, eval cases, docs, and features are welcome. Installing Wenlan does not require building from source. For local development, run these commands from this repository's root:

```bash
# daemon crates (default-members — the desktop app is not compiled)
cargo build
cargo test

# desktop app (Cargo target and root-level frontend tooling)
pnpm install
pnpm dev:all
pnpm build:all
```

`pnpm dev:all` is the supported development entry point for the desktop app. It keeps development ports, data, process ownership, app identity, MCP sockets, and Remote Access state separate from the installed production runtime; a debug app started without that isolation refuses to run. See this repository's [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](.github/CONTRIBUTING.md), plus the in-tree [app/AGENTS.md](app/AGENTS.md), for the complete development workflow. Security reports: [SECURITY.md](.github/SECURITY.md). Privacy policy: [PRIVACY.md](docs/PRIVACY.md). Please also read the [Code of Conduct](.github/CODE_OF_CONDUCT.md).


<a id="code-signing-policy"></a>

## Code signing policy

macOS desktop releases are Developer ID signed and notarized. Windows installers are not signed yet.

Releases are built from tagged commits by this repository's release workflow on GitHub-hosted runners. Maintainers must use multi-factor authentication on GitHub.

[Platform signing details](docs/code-signing.md) · [Privacy policy](docs/PRIVACY.md).


<a id="license"></a>

## License

Wenlan uses two licenses, one per part of the repository.

- **Apache-2.0** ([`LICENSE`](LICENSE)) covers the local runtime, CLI, MCP server, shared types, and the Claude Code and Codex plugin files. Build on these freely.
- **AGPL-3.0-only** ([`app/LICENSE`](app/LICENSE)) covers the desktop app: the `app/` crate and the React frontend it ships. If you run a modified version of the app as a network service, the AGPL asks you to offer that modified source to its users.

The split is deliberate. Apache-2.0 code may be used inside an AGPL-3.0 program, so the desktop app builds on the runtime without either license being violated.


<a id="acknowledgments"></a>

## Lineage and peers

Wenlan (文瀾) takes its name from 文瀾閣, an imperial library that held 四庫全書 as part of one of China's largest book collections.

Wenlan's llm-wiki v2 model is its own product direction, informed by the LLM-wiki and agent-memory lineages:

- [Karpathy's LLM-wiki note](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) established the raw-source-to-maintained-wiki pattern.
- [Rohitg00's LLM Wiki v2 proposal](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2) extends that pattern with memory lifecycle, confidence, graph, and retrieval mechanisms. [agentmemory](https://github.com/rohitg00/agentmemory) is its concrete agent-memory implementation.
- [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki) is a full desktop implementation of the document-centered LLM-wiki pattern.
- [basic-memory](https://github.com/basicmachines-co/basic-memory), [obsidian-mind](https://github.com/breferrari/obsidian-mind), [mcp-memory-service](https://pypi.org/project/mcp-memory-service/), [Memoria](https://github.com/matrixorigin/Memoria), and [OpenMemory](https://github.com/CaviraOSS/OpenMemory) explore adjacent local knowledge and agent-memory shapes.
