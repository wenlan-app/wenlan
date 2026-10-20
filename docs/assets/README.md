# README visual assets

## Preserved concept illustrations

The Agent Loop overview is retained for future reuse, although the main READMEs now use the approved compositions below in its place:

- `wenlan-system.svg` / `.png`: English desktop overview.
- `wenlan-system-zh-Hant.svg` / `.png`: Traditional Chinese desktop overview.
- `wenlan-system-zh-Hans.svg` / `.png`: Simplified Chinese desktop overview.
- Matching `*-mobile.svg` / `.png` versions remain alongside them.

The original knowledge-network illustrations (`wenlan-knowledge-network*`) are also retained. The AI-work comparison (`wenlan-knowledge-to-work*`) is retained but no longer shown in the READMEs; the retrieval text stays. These are explanatory illustrations, not captures of the app.

Their editable source is `scripts/readme-product-visuals.cjs`; the renderer and font setup are documented in [the visual toolchain](../../scripts/readme-visuals/README.md). Removing an illustration from the README does not retire its assets or generator.

## Current product captures

See [screenshots and provenance](screenshots/README.md). Product captures use fictional content and the actual frontend, without compositing UI elements.

## Approved README compositions

The AI-work illustration is fully localized for English, Traditional Chinese, Simplified Chinese, and Spanish, including the Claude conversations, retrieval cards, graph labels, and finished homepage. The panorama, personal graph, and update-history compositions have English, Traditional Chinese, and Simplified Chinese versions. Their approved composition, demo content, nodes, and relationships are retained. The panorama and personal graph translate captured text using the original DOM/styles and text layers; they are localized composites, not newly captured native-language UI. The update/history artwork is a native SVG illustration, not an App screenshot. Spanish versions of these three figures are deferred.

The final consistency pass aligns the three explanatory titles (46px, semibold, shared left edge), preserves native type icons and colors, and moves provenance footnotes into readable README captions. It preserves approved body composition and content. The video cover uses a shortened demo article captured in the App, an enlarged native memory preview, and a larger native directory crop. Graph labels and spacing are balanced for README width; provenance stays in the caption beneath the image.

| Asset | Export | Provenance |
| --- | --- | --- |
| `wenlan-personal-knowledge.png`, `-zh-Hant.png`, `-zh-Hans.png` | 6000 × 3600 | Actual App directory/header captures and a high-resolution render of the actual editor and memory-preview DOM/styles, combined with an editorial graph layout. All 33 graph nodes and 80 typed relationships retain their verified demo identities. Not a native three-column screen; App behavior is unchanged. |
| `wenlan-personal-graph.png`, `-zh-Hant.png`, `-zh-Hans.png` | 3600 × 2062 | Actual graph capture; the original nodes and connections are preserved, with localized legend and text layers. Region names and boundaries are editorial annotations, not verified source-node identities or detected communities. |
| `wenlan-knowledge-to-work.png`, `-zh-Hant.png`, `-zh-Hans.png`, `-es-ES.png` | 5400 × 3480 each | Fictional Claude conversations and FORMA homepage, using the film’s unchanged hero-image asset. Keyword, semantic, and graph paths point to the matching headline, primary action, and image. Concept illustration, not a measured comparison; Wenlan context complements Claude’s native capabilities. |
| `wenlan-updates-history.png`, `-zh-Hant.png`, `-zh-Hans.png` | 4800 × 2160 | Native SVG explanatory illustration synchronized unchanged from the website artwork. A source change enters Steep and informs versioned memories and automatic updates to eligible notes; user-created or edited notes await review during background refresh. Not an App screenshot. Matching SVG files preserve editable text and geometry. |

The personal graph omits its former in-image heading and subtitle because the README section already introduces it. Only that header and its reserved space were removed; the graph image, labels, nodes, connections, and geometry are unchanged. Editable sources and 900px proofs are retained in `graph-without-heading-20261017`. Spanish continues to use the English graph.

The graph uses the website's September 6, 2026 capture, `wenlan-live-knowledge-graph-20260906.webp` (source SHA-256 `63ab14b444c9cfd8527f9fc80440d7fb2507856b745c1d83b10e52e61df0b5f9`). Original graph nodes, colors, and connections remain intact. Urban history, Genetics and evolution, and Human–AI collaboration research are illustrative region names.

The localization sources are retained in the `readme-locales-final` production package. Panorama exports are 6000 × 3600; graph exports are 3600 × 2062; update-history exports are 4800 × 2160. Each was inspected at 900px reading width. The Traditional Chinese panorama remains byte-identical to its approved version; its graph changes only the native English legend and footer text to Traditional Chinese. Longer translations wrap inside the existing composition.

Composition sources: `cover-polish-title` (6000px `cover-polish-hd` master; approved `cover-polish-v2` layout), `real-graph-polish-v7`, `ai-source-card-type-unified-20261017` / `ai-work-publish-locales-20261017`, and the website’s `render-update-visuals.cjs` / `shared-updates-history-20261017` for the current history artwork (`lifecycle-polish-v26` is the previous composite), retained in their corresponding production packages, including `cover-polish-title`, `cover-polish-hd`, `readme-final-consistency`, and `ai-work-film-case-v7-localized`. The final title pass reduces only the article heading from 28.16px to 26px; every pixel outside that heading area is identical to the 6000px master. The cover package includes read-only snapshots of the actual editor and hover-preview DOM/styles, original pseudo-element icon CSS, the approved composition, export instructions, and SHA-256 verification. Text and graph labels are rendered at high resolution rather than enlarged from the former small article crop. Heading font fallback is normalized to match the approved capture. Directory/header raster inputs remain unchanged; their detail is limited by the original captures. Browser capture uses JPEG transport; the published PNG adds no further lossy encoding. No generative enhancement or invented UI content is used in the product cover. The AI-work comparison remains an authored concept as noted above.

The older `wenlan-lifecycle*` and `wenlan-knowledge-network*` illustrations and their generators remain available for reference; the current READMEs do not use their old mobile variants.

### Current export hashes

- `wenlan-personal-knowledge-zh-Hant.png`: `328e4bf032d5dcf16cad24658d7d12a51d694fdd2b3d00f5b1e44f94b22246e5`
- `wenlan-personal-graph-zh-Hant.png`: `8cbb19f01f55dd27234bb53c4852e590209052ff9914fbaa9bd8f4f0def4023c`
- `wenlan-knowledge-to-work-zh-Hant.png`: `c8e0e70a1ba4c502b5d48fada885074305761f35fba892c50ea612a2a78a9352`
- `wenlan-knowledge-to-work-zh-Hans.png`: `b8aea4a7fcda8cb3e5e7230a010182d49cfe5e6d538e1c7ba71e2019e8100107`
- `wenlan-knowledge-to-work.png`: `14e34fb772ce8f4ac6ce20516f136b9c1aead3fdef7304426445ebc3009307e6`
- `wenlan-knowledge-to-work-es-ES.png`: `10b7886bedcfbac8b62d8efdb047d66831319664d7878b0193790cd5cd9e33fd`
- `wenlan-updates-history-zh-Hant.png`: `bdbf253296b9962500c58bbfc5959246ca88f17bcc49ddf9e2c56d81a3325949`
- `wenlan-personal-knowledge.png`: `6aa9bd21bb7e5b746ec1c1545570f0f5df9539fb81ffcf4dc1f915295b2de0e9`
- `wenlan-personal-knowledge-zh-Hans.png`: `96736638a3611964a0b63888e41777f98d07f93e8f9db7b39c964df43dc4eeba`
- `wenlan-personal-graph.png`: `17d65ff5749464c96f7a5b34ffa2e780a0b5d7fd81f90b45107d474b8242e5e0`
- `wenlan-personal-graph-zh-Hans.png`: `e9936e86d7550404c4886c91e1a0cec9436c2252fa8c3be958188e0fa8c1fc11`
- `wenlan-updates-history.png`: `b8674e6f37f2178263e65d97a732aaa9c2a19cc5424086ef50e076de50fb93a0`
- `wenlan-updates-history-zh-Hans.png`: `0bf421529d9eda0b3cced0e3a57fc6c74b0b1746cd8cdd8004048ff7919a5e42`

## Neutral background palette

The daily workflow, architecture, and AI-work compositions use the approved neutral background direction: `#FCFCFC` canvas, neutral `#F4F5F5` panels, and white cards. Warm editorial background fills are neutralized; existing text, accents, semantic highlights, and native App screenshot colors are retained. The already-neutral panorama and personal graph remain byte-identical, as does the dark banner.

The palette pass preserves the approved PNG dimensions and original text/geometry pixels. Background masks come from before/after renders of the existing code-native sources; only matching light background pixels are changed, without resampling the approved raster. The earlier history composite retained all 11 native capture regions pixel-identically; it has since been replaced by the website’s native SVG illustration. Four-language variants were inspected at 900px README width; Spanish retains the previously approved fallback images where localized artwork was deferred. Source, masks, original exports, and verification are retained in the `readme-neutral-20261017` production package. The current history illustration is copied exactly from the website’s `vector-rebuilt-20261017` assets (source revision `ffe29d62f083010bea1ee7f78cb44fd6b1f4b61b`), including its `#FCFCFB` canvas. Its predecessor remains in `shared-visual-palette-20261016`.

## Architecture overview

`wenlan-architecture.png`, `-zh-Hant.png`, `-zh-Hans.png`, and `-es-ES.png` are fully localized architecture illustrations, exported at 4800 × 3300. Matching SVG files are the editable vector sources. They are diagrams, not App screenshots. The approved Traditional Chinese composition is preserved; longer translated labels use concise wording and local line wrapping.

The diagram separates imported materials, App/CLI and AI-tool access, the local service, and persistent knowledge. It distinguishes local search models, the external AI client’s own model, and optional background enrichment models. Page citations, dependent-page invalidation, review for human-owned pages, and version history are represented at product level, not as a complete database schema or a guarantee that every retrieval uses every channel.

Source references: [setup and model roles](../setup-and-data.md#models), [retrieval and connected knowledge](../technical-foundations.md), [source invalidation](../../crates/wenlan-core/src/db.rs), and [Page ownership and proposed revisions](../../crates/wenlan-core/src/post_write/page_update.rs). Rendered at 3× from SVG and checked at 900px README width. Existing approved product figures are unchanged.

## Daily workflow

`wenlan-daily-workflow.png`, `-zh-Hant.png`, `-zh-Hans.png`, and `-es-ES.png` are fully localized concept diagrams, exported at 3984 × 2256. Matching SVG files preserve editable text and geometry. The Traditional Chinese export is the approved V13 master; other locales retain its layout, icons, colors and connections, with localized line wrapping.

Sources and existing memories enter a shared local library. App access is bidirectional. AI tools read shared knowledge through a plugin/MCP connection and save memories, session records and progress back. `/capture`, `/handoff`, and `/brief` are optional actions in AI work, not a mandatory sequence. Handoff does not imply automatic transcript import or page synthesis. Knowledge-page organization and updating remain optional.

Source contracts: [capture](../../plugin/skills/capture/SKILL.md), [handoff](../../plugin/skills/handoff/SKILL.md), [brief](../../plugin/skills/brief/SKILL.md), and [knowledge guide](../knowledge-guide.md). All four locale exports were inspected at 830px reading width. The existing README images are unchanged.

## Banner wording and retrieval-graph finish

Banner headlines omit terminal punctuation. The brand uses Wenlan, with 文瀾 / 文澜 in Chinese variants. Desktop banners use a 1120 × 260 canvas exported at 3×; mobile exports remain 2160 × 768. Font stacks match the R126 film: SF Pro Display / Helvetica Neue for Latin, PingFang TC / SC with Noto Sans fallbacks for Chinese. The centered logo-and-copy group retains left-aligned copy and unchanged slogans. Sources and 900px/480px proofs are retained in `readme-banner-film-aligned-20261017`. Chinese uses 個人知識庫 / 个人知识库; English retains personal wiki. The latest AI-work pass retains the Claude comparison, three retrieval routes, graph relationships, and finished homepage. Source-card titles sit on the left and their native-colored type icons on the right; aligned search cues and restrained highlights distinguish queries from matching content. The Traditional Chinese PNG is byte-identical to the approved `ai-source-card-type-unified-20261017` export. English, Simplified Chinese, and Spanish retain the composition with localized line wrapping and route endpoints. All four 5400px exports were inspected at 900px; editable HTML, the unchanged film hero asset, and proofs are retained in `ai-work-publish-locales-20261017`. The earlier graph-only pass remains in `readme-graph-detail-20261017`.
