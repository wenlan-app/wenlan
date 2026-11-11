<!-- README_SYNC: source=README.md sha256=26a45caeecaec366b2b05e4b6c7a454619cbc4af00ecc46130690a5a91d7e0e3 -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-zh-Hans-mobile.png">
    <img src="./docs/assets/readme-banner-zh-Hans.png" alt="文澜：不用你维护的知识库。免费、开源、本地优先。" width="100%">
  </picture>
</p>

<p align="center">
  <strong>想写就自己写，其他交给 AI 整理。</strong><br>
  开源的 Obsidian 替代品，也能直接接上你现有的仓库。一键连上你的 AI 工具，不用自己装插件、维护插件。<br>
  还能跟团队共用。笔记是你电脑上的文件，只会去你让它去的地方。
</p>

<p align="center">
  <a href="./README.md">English</a> | 简体中文 | <a href="./README.zh-Hant.md">繁體中文</a> | <a href="./README.es-ES.md">Español</a>
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><strong>下&#8288;载</strong></a> ·
  <a href="#start-in-30-seconds">开&#8288;始&#8288;使&#8288;用</a> ·
  <a href="#what-can-i-bring-in">来&#8288;源</a> ·
  <a href="#learn-more">文&#8288;档</a> ·
  <a href="https://discord.gg/nmzVTTU3HG">Discord</a>
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="最新版本" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?label=release"></a>
  <a href="#license"><img alt="许可证：Apache-2.0 与 AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<p align="center">
  <a href="https://youtu.be/vM2GXS6UjyA"><img src="./docs/assets/readme-video-cover-zh-Hans.png" alt="观看 Wenlan 介绍视频（1:44，英文）" width="100%"></a>
</p>

<a id="why-wenlan"></a>

<br>

## 为什么用文澜

我做文澜，是因为笔记、文档和 AI 对话散落各处，要让笔记系统不荒废得不停维护。文澜是给每天用 AI 工具、又不想维护笔记系统的人。

- **改了什么都看得到：** 它改了什么你都看得到，可以还原，也可以决定哪些不让它改。每个引用都能点回原文，引用大多对不上来源的笔记不会保存。
- **想写就自己写：** 随时打开一页就能写。你写的笔记设成“先问我”。
- **其他交给 AI 整理：** 你平常用的 AI 工具把你的笔记、文档和 AI 对话写成笔记、连起来，来源变了就跟着更新。文澜不自带 AI。
- **一键连上：** 在设置里点“全部连接”，就连好你电脑上的 AI 工具。不用自己找插件、装插件、维护插件。
- **跟团队共用：** 把一个 Space 放进 Google Drive 或 iCloud 云盘的文件夹共用，同事在自己的电脑上加入就能用。
- **带进现有资料：** 笔记、文档、链接、对话和 Obsidian 仓库都能带进来（[完整清单](#what-can-i-bring-in)），存成本地 Markdown 文件。

<a id="how-it-compares"></a>

### 跟其他做法比

| | Obsidian 加插件 | 让 AI 工具直接改 Markdown 文件夹 | 文澜 |
|---|---|---|---|
| 谁来整理 | 你自己，加上你找来、要维护的插件 | 你的 AI，每次都要你开口 | 你的 AI 工具边做边整理，来源变了会再更新 |
| 设置 | 找插件、装插件、更新插件 | 自己写指示 | 在设置里点“全部连接”，一键完成 |
| 检查 AI 写的东西 | 看插件 | 每个改动都要自己看 | 每个引用都能点回原文；引用大多对不上的笔记不会保存 |
| AI 能改什么 | 看插件 | 文件夹里什么都能改 | 每篇笔记由你决定，也能还原到之前的版本 |
| 你现有的仓库 | 本来就是你的仓库 | 让 AI 指向它 | 直接接上；笔记留在原处 |

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>
<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>
<a id="claude-code-in-30-seconds"></a>
<a id="codex-plugin"></a>
<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

<br>

## 开始使用

### 1. 下载并打开 Wenlan

[下载桌面版](https://github.com/wenlan-app/wenlan/releases/latest)，支持 macOS 11 以上的 Apple Silicon 和 Windows x64。App 界面有英文、简体中文和繁體中文。

更喜欢终端？在 Mac 上运行 `npx -y wenlan setup`，或运行 `brew install 7xuanlu/tap/wenlan` 再运行 `wenlan setup`。Linux 和 Windows 请看 [CLI 安装说明](docs/setup-and-data.zh-Hans.md#installation)。

### 2. 带入你的资料

新建一篇笔记，或带入 Obsidian 仓库、Logseq 图谱、文件夹或之前的 AI 对话。[导入指南](docs/import-guide.md)。

### 3. 连接你的 AI 工具

在 App 的设置里，“添加工具”会找出你电脑上的 AI 工具，点“全部连接”一次设好，不用另外装东西或更新。接着开一个新的 AI 会话，问它：

> /recall 我之前对这件事的笔记和决定，再帮我：……

想自己手动设置，或看看会连上什么：

| AI 工具 | 会设置什么 | 说明 |
|---|---|---|
| Claude Code / Claude Desktop | 插件 / MCP | [Claude 指南](docs/setup-with-ai.md#claude-code) |
| ChatGPT Desktop（Codex）、Codex CLI | 插件 | [Codex 指南](docs/setup-with-ai.md#codex) |
| Cursor、GitHub Copilot、VS Code、Gemini CLI、Antigravity、Windsurf、Kiro、OpenCode、Kimi Code | MCP 和技能 | [MCP 指南](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |
| Muse Code 和其他 MCP 工具 | MCP | [MCP 指南](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |
| Grok Bot | 本机 CLI，或 MCP | [Grok Bot 指南](docs/setup-with-ai.md#grok-bot) |

Codex 中用 `$recall`；只用 MCP 的工具，直接请它先查文澜。Grok Bot 在你的电脑上运行 `wenlan recall`，或在那里开一段短暂的 `wenlan-mcp` 会话（[Grok Bot 指南](docs/setup-with-ai.md#grok-bot)）。

导入完成后，App 会提供“用（你的 AI 工具）写页面”，把资料写成第一批带引用的笔记，不用输入指令。

<a id="what-can-i-bring-in"></a>
<a id="sources-and-imports"></a>

<br>

## 能带进什么

| 可以带入什么 | 导入后怎么用 |
| --- | --- |
| Markdown、文本文件、文本型 PDF、DOCX 或文件夹 | 搜索、引用里面的文字。文件夹可以只导入一次，也可以持续同步文件的变化。 |
| Obsidian 仓库或 Logseq 图谱 | 搜索和引用现有笔记。选择持续同步，在后台跟上变化；或只导入一次。你写的笔记默认是“先问我”：AI 的修改会变成建议，由你接受或不要；每篇笔记都可以改这个设置。 |
| 网页链接 | 在笔记里粘贴链接，就会保存为来源。YouTube、X、Threads、Instagram 和小红书的链接会显示成卡片、可播放的视频或原始帖子。 |
| Gmail、Notion、Google Drive、Slack、GitHub | 你的 AI 工具用它自己的连接器读取，把重要的内容存进文澜，并附上原文链接。 |
| 你的 AI 对话 | 你的 AI 工具会把每次对话里的决定、发现和进度存成记忆，不会存完整对话记录。ChatGPT 或 Claude 导出的 ZIP 可以带入之前的对话，保留导入当时的内容。 |
| 代码仓库 | `/wenlan wiki` 写出对应到代码行的笔记。[代码 wiki](#code-repository-wiki)。 |

也可以把 Open Knowledge Format（OKF）文件夹连接为来源。扫描版 PDF 需要先提取文字；对话附件不会导入。[导入指南](docs/import-guide.md)。

<a id="what-makes-wenlan-distinct"></a>
<a id="why-is-wenlan-different"></a>
<a id="two-lifecycles"></a>
<a id="two-lifecycles-one-maintained-knowledge-system"></a>
<a id="local-markdown"></a>
<a id="atomic-memory"></a>
<a id="maintained-page"></a>
<a id="local-markdown-that-works-with-obsidian"></a>
<a id="keep-knowledge-current-and-preserve-history"></a>
<a id="steep-keep-knowledge-useful-as-it-changes"></a>
<a id="what-it-does-on-its-own"></a>
<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>
<a id="what-is-this"></a>
<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="knowledge-graph"></a>
<a id="browse-and-organize-your-knowledge"></a>
<a id="connect-your-knowledge"></a>
<a id="organize-scattered-material-into-knowledge"></a>
<a id="explore-your-personal-knowledge-graph"></a>
<a id="daily-workflow"></a>
<a id="in-everyday-use"></a>
<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>
<a id="use-your-knowledge-with-ai"></a>
<a id="put-your-knowledge-to-work-with-ai"></a>
<a id="if-you-already-use-claude-code-on-your-notes"></a>

<a id="两套生命周期一个持续维护的知识系统"></a>
<a id="原子记忆"></a>
<a id="持续维护的页面"></a>
<a id="与-obsidian-共存的本地-markdown"></a>
<a id="持续更新知识并保留历史"></a>
<a id="这是什么"></a>
<a id="浏览和整理个人知识"></a>
<a id="让知识彼此连接"></a>
<a id="探索你的个人知识图谱"></a>
<a id="从关键词语义与关联找回正确内容"></a>
<a id="用-ai-延续已有知识"></a>
<a id="让-ai-接续已有知识"></a>
<br>

## 它怎么帮你整理

你的 AI 工具会读你带进来的资料，写成 Markdown 笔记：把相关的笔记连起来，标出每句话的出处，来源变了就跟着更新。任何一篇笔记你都可以打开自己写。

<p align="center">
  <img src="./docs/assets/wenlan-updates-history-zh-Hans.png" alt="示意流程：文件来源中的发现将记忆由 V1 更新为 V2，再影响自动更新或待审修订的笔记。" width="100%">
</p>
<p>示意图依据当前的更新行为绘制；并非 App 截图。</p>

- **每篇笔记都有出处：** 每个引用都能点回原文。笔记用 `[n]` 引用来源时，文澜会拿每个引用的句子对照它的来源；引用大多对不上时，不会保存该笔记。通过检查不代表内容一定正确。
- **跟着来源更新：** 来源、记忆或引用的代码变了，依据它们的笔记会被标出来等待更新。不需要任何模型，文澜也会建议合并近似重复的笔记，并列出还没有对应笔记的 `[[链接]]`。
- **AI 能改什么由你决定：** 你写的笔记默认是“先问我”，AI 的修改会先等你点头。每篇笔记都可以设成“直接改”或“先问我”，看得到改了什么，也能还原到之前的版本。
- **两种方式：** “随叫”是默认，由你的 AI 工具在工作时顺手写，不用另外设置 API key。“自动”则由文澜用你选的模型在后台处理，模型可以在你的电脑上，也可以在云端。[沉淀指南](docs/steep.md)。
- **在你的 AI 工具里：** `/recall` 找回之前的笔记和决定，`/capture` 存下一个决定或心得，`/handoff` 记下做到哪里，`/brief` 下次接着做。[插件指令](plugin/skills/README.md)。
- **在 App 里：** 不连 AI 也能加资料、写笔记、改笔记、查回引用、审核修改建议。笔记可以包含 Mermaid 图表，由 App 画出来。

已经用 Claude Code 或 Codex 管一个笔记文件夹？继续用就好。文澜接上同一个工具，补上文件夹自己做不到的事：检查引用、来源变了会标记，而且每篇笔记 AI 能不能改由你决定。

<a id="code-repository-wiki"></a>
<a id="a-wiki-for-your-code-repository"></a>

<br>

## 为代码仓库建 wiki

在代码仓库中运行 `/wenlan wiki`（Codex 中为 `$wenlan wiki`）。你的 AI 工具会写一篇总览，再为新人需要了解的每个部分或流程各写一篇。再次运行时只补写缺少的笔记。

- **与代码绑定：** 每篇笔记都会记录它依据的代码行；代码变了，笔记会显示改了什么。
- **以 pull request 放进仓库：** Space 上的 GitHub 按钮会把笔记以 pull request 放进仓库的 `wiki/` 文件夹，可以点按钮运行，也可以每天自动跑。每日更新可以在你的电脑上用你自己的 AI 工具跑，或在 GitHub Actions 上跑（AI 工具的密钥存为仓库 secret）。同一个 pull request 还会修正仓库文档中与代码不再相符的句子。
- **保留你的修改：** 有人在仓库中修改过的 wiki 笔记会保留那次修改。引用你的记忆或对话的笔记不会放进仓库。

<a id="what-wenlan-is-not"></a>
<a id="retrieval"></a>
<a id="retrieval-across-words-meaning-and-connections"></a>
<a id="architecture"></a>

<br>

## 给技术读者

**共用本地知识库。** App、CLI 与连上的 AI 工具通过文澜的本地服务读写。笔记以 Markdown 文件保存；记忆、关系与搜索索引存在本地 libSQL。本地服务、CLI 和 MCP server 用 Rust 写成；桌面 app 用 Tauri，界面是 React 和 TypeScript。

<p align="center">
  <img src="./docs/assets/wenlan-architecture-zh-Hans.png" alt="架构示意：App 与 AI 工具共用本地知识库，后台整理模型为可选项。" width="100%">
</p>

- **检索：** 用 SQLite FTS5 找出相同词句、BGE 向量找出相近语义（libSQL 的 DiskANN 向量索引），再以加权 RRF 合并排名；基础搜索不需要 API key。共同实体会提高相关记忆的排名，另可启用 cross-encoder reranking 精排。选定 Space 就只返回该项目的内容。
- **图谱：** 配置整理模型后，实体抽取与实体消歧会把不同来源里提到的同一人物、项目或概念连起来；用 Label Propagation（默认）或 Leiden 把相关主题分组。
- **记忆：** 发现存成一条一条独立的小记忆，不必载入整段对话或整篇笔记就能找回。

[检索、引用与模型](docs/technical-foundations.md)。

<a id="evaluation"></a>

### 评估

这些 LongMemEval（LME）测量结果评估的是已存知识的检索，不是 AI 回答质量。Recall@5 表示相关资料出现在前五条结果中的比例。

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->

一万篇笔记的测试（生成的测试笔记，M2 Pro 16 GB，v0.20.0）：App 搜索 11 到 50 毫秒，重启不到 0.3 秒，内存峰值约 1.2 GB。0.20.1 修好搜索后，AI 工具在同一个资料库里找到了全部 30 个罕见测试字词。

在 0.20.2 导入一万篇笔记的 Obsidian 仓库（虚构的笔记，M2 Pro Mac 16 GB，debug build，由 Claude Code 写笔记）：15 秒后可以搜索，约 2 分钟写出第一篇笔记，约 4 分钟写出 8 篇带引用的笔记。

[评估方法与基准详情](docs/eval/README.md)。

<a id="models-and-privacy"></a>
<a id="data-and-privacy"></a>

<br>

## 数据与隐私

笔记是你电脑上的 Markdown 文件，符合条件的笔记可以导出为 Open Knowledge Format（OKF）知识库。云端 AI 工具可能会把找回的内容传给它的服务商，云端模型也会收到它处理的内容，所以本地存储不代表这些交互都留在本机。使用统计默认关闭。

**网页访问（实验中）：** 在“设置”中开启“网页访问”，就能让 Claude、ChatGPT 或 Muse 等网页 AI 客户端，在你的电脑保持在线时搜索整个资料库或指定空间。允许应用时可以选“只能搜索”或“搜索和写入”；可以写入的应用还能新增、修改和删除内容，AI 应用可以在每次改动前先问你。查询与结果会经过中继服务，详见[隐私边界](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector)。

[文件位置、备份与移除](docs/setup-and-data.zh-Hans.md#backup-and-removal) · [联网与隐私详情](docs/PRIVACY.md#when-wenlan-reaches-the-network)。

<a id="learn-more"></a>
<a id="whats-new"></a>
<a id="离线队列outbox"></a>

<br>

## 进一步了解

- [开始使用与安装](docs/setup-and-data.zh-Hans.md#installation)
- [连接 AI 工具](docs/setup-with-ai.md)
- [来源与导入](docs/import-guide.md)
- [本地文件、备份与离线使用](docs/setup-and-data.zh-Hans.md)
- [隐私](docs/PRIVACY.md)
- [Doctor 与问题反馈](https://wenlan.app/docs/diagnostics-and-issue-reports)
- [每次发布的更新记录](CHANGELOG.md)
- [完整文档索引](docs/README.md)

<br>

## 贡献

欢迎加入 [Discord 社区](https://discord.gg/nmzVTTU3HG)。请在 [GitHub issues](https://github.com/wenlan-app/wenlan/issues) 反馈问题或提出功能建议；Issue 公开可见，分享前请遮盖密钥和个人资料。

代码与文档贡献请见[贡献指南](.github/CONTRIBUTING.md)、[安全问题反馈](.github/SECURITY.md)和[行为准则](.github/CODE_OF_CONDUCT.md)。

<a id="code-signing-policy"></a>

<br>

## 代码签名政策

请见[各平台签名与公证说明](docs/code-signing.md)。

<a id="license"></a>

<br>

## 许可

本地 runtime、CLI、MCP server、shared types 和 plugins 使用 [Apache-2.0](LICENSE)。桌面 app 及其前端使用 [AGPL-3.0-only](app/LICENSE)；修改版若作为网络服务运行，须向用户提供其源代码。

<a id="acknowledgments"></a>

<br>

## 源流与同类项目

Wenlan 名称来自文澜阁，产品方向承接 LLM-wiki 与 agent-memory 脉络：[Karpathy 的 LLM-wiki note](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)、[Rohitg00 的提案](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2)和 [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki)。
