<!-- README_SYNC: source=README.md sha256=67cdf91d8405178c43a6a0837eeed71f1284a8b6ad041569773b85f31c61abfb -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-zh-Hans-mobile.png">
    <img src="./docs/assets/readme-banner-zh-Hans.png" alt="Wenlan：持续更新的知识库。免费、开源、本地优先。" width="100%">
  </picture>
</p>

<p align="center">
  <strong>笔记、代码、AI 对话整理成知识库，对照过来源。</strong><br>
  你和 AI 工具共用同一份资料，来源变了会跟着更新。<br>
  你的笔记留在你的电脑上，只会发给你选的 AI 工具。
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

我做文澜，是因为笔记、文档和 AI 对话散落各处又一直在变，要让笔记库不荒废得不停维护。我想要一个 AI 原生、不用费心照顾的知识库。

- **你的 AI 帮你写、帮你维护：** 你平常用的 AI 工具把你的文档、旧笔记和 AI 对话写成新的笔记，不用另外装模型。
- **站不住的会告诉你：** 引用大多对不上来源的笔记不会保存，每个引用都能点回原文。
- **变了会标出来：** 来源、笔记或代码一变，只有依靠它的笔记会被标出来。
- **你写的还是你的：** 没有你同意，不会改动你原有的笔记；你写的笔记设成“先问我”。
- **带进现有资料，换工具也不怕：** 笔记、文档、链接和对话都能带进来（[完整清单](#what-can-i-bring-in)），存成本地 Markdown，你选的 AI 工具都能用。

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

| AI 工具 | 连接方式 | 设置入口 |
|---|---|---|
| Claude Code / Claude Desktop | 插件 / MCP | [Claude 指南](docs/setup-with-ai.md#claude-code) |
| ChatGPT Desktop（Codex）、Codex CLI | 插件 | [Codex 指南](docs/setup-with-ai.md#codex) |
| Cursor、GitHub Copilot、VS Code、Gemini CLI、Antigravity、Windsurf、Kiro、OpenCode、Kimi Code | MCP 和技能 | [MCP 指南](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |
| Muse Code 和其他 MCP 工具 | MCP | [MCP 指南](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |

在 App 的设置里，“添加工具”会找出你电脑上的 AI 工具，点“全部连接”一次设好。接着开一个新的 AI 会话，问它：

> /recall 我之前对这件事的笔记和决定，再帮我：……

Codex 中用 `$recall`；只用 MCP 的工具，直接请它先查文澜。

导入完成后，App 会提供“用（你的 AI 工具）写页面”，把资料写成第一批带引用的笔记，不用输入指令。

<a id="what-can-i-bring-in"></a>

<br>

## 来源与导入

| 导入内容 | 用途 |
| --- | --- |
| Markdown、文本、文本型 PDF、DOCX 或文件夹 | 搜索和引用导入文本。文件夹可以只导入一次，也可以持续同步文件的变化。 |
| Obsidian 仓库或 Logseq 图谱 | 搜索和引用现有笔记。选择持续同步，在后台跟上变化；或只导入一次。没有你同意，不会改动你原有的笔记：在 Obsidian 仓库里，设成“先问我”的笔记会收到 AI 的修改建议，由你接受或不要。 |
| 网页链接 | 在笔记里粘贴链接，就会存成来源。YouTube、X、Threads、Instagram 和小红书的链接会显示成卡片、可播放的视频或原始帖子。也可以粘贴文本与 URL，保存那段摘录。 |
| Gmail、Notion、Google Drive、Slack、GitHub | 你的 AI 工具用它自己的连接器读取，把重要的内容存进文澜，并附上原文链接。 |
| 代码仓库 | `/wenlan wiki` 写出对应到代码行数的笔记。[代码 wiki](#code-repository-wiki)。 |
| OKF 文件夹 | 把 Open Knowledge Format 文件夹（例如 OpenWiki 或文澜的导出）连接成来源。 |
| 工作中的 AI 对话 | Claude Code、Codex 和你的其他 AI 工具，会把每次对话里的决定、发现和进度存成记忆，笔记再引用它们。不会导入完整对话记录。 |
| ChatGPT 或 Claude 导出 ZIP | 检索过去的对话文本（包括最后一条没有得到回复的消息）并作为上下文引用；导入是快照，不会连接实时账户。 |
| 文本形式的现有记忆 | 将偏好、决定或经验保存为单条记忆。 |

扫描版 PDF 需先提取文本；聊天附件不会导入。[导入指南（英文）](docs/import-guide.md)。

<a id="what-makes-wenlan-distinct"></a>
<a id="why-is-wenlan-different"></a>
<a id="two-lifecycles"></a>
<a id="two-lifecycles-one-maintained-knowledge-system"></a>
<a id="两套生命周期一个持续维护的知识系统"></a>
<a id="local-markdown"></a>
<a id="原子记忆"></a>
<a id="持续维护的页面"></a>
<a id="与-obsidian-共存的本地-markdown"></a>

<a id="持续更新知识并保留历史"></a>

<br>

## Steep（沉淀）：让知识跟上新发现

<p align="center">
  <img src="./docs/assets/wenlan-updates-history-zh-Hans.png" alt="示意流程：文件来源中的发现将记忆由 V1 更新为 V2，再影响自动更新或待审修订的笔记。" width="100%">
</p>
<p>示意图依据当前的更新行为绘制；并非 App 截图。</p>

保存资料只是开始。**Steep 会整理资料以便重用，并让受支持的笔记保持更新**，让你和 AI 能接续先前的发现，而不必反复重读原始材料。

在“设置”中选择两种方式之一：

- **随叫**（新安装默认）：AI 工具在文澜里保存或写入时，会顺手沉淀，并在工作时得知哪些笔记过期、哪些资料还没写进笔记。不用下指令；想一次全部整理，也可以运行 `/wenlan steep`。
- **自动：** 文澜用你指定的模型在后台沉淀。

沉淀会做这些事：

- **整理记忆：** 添加类型、结构化细节、搜索线索和图谱链接，让单条发现更容易在上下文中检索。
- **让依据带动更新：** 来源、记忆或引用的代码变化时，依赖它们的笔记会标记待刷新；新记忆也可以丰富符合条件的笔记。如果草稿未通过引用检查，刷新会保留已保存笔记。通过检查并不代表事实正确。
- **保留控制权与历史：** 后台刷新会更新符合条件的 AI 维护笔记；你撰写或编辑的笔记会收到修订提案。明确替代记忆时，旧记忆会隐藏或归档，同时保留其链接。笔记修订可供检查。
- **先问我：** 开启“我写过的笔记：先问我”（新安装默认开启）后，AI 对你写过或改过的笔记所做的修改会先等你同意。每篇笔记的“AI 修改”设置会显示“直接改”或“先问我”。AI 写的笔记照常更新。

<a id="what-it-does-on-its-own"></a>

### 它自己会做的事

两种模式都会做，不需要模型：

- **标出过期的笔记：** 来源、记忆或引用的代码变了，相关笔记会被标记。代码 wiki 的笔记会显示代码改了什么。
- **建议合并近似重复的笔记：** 生成一张合并卡，由你或你的 AI 决定接受或忽略。
- **整理链接：** 列出还没有对应笔记的 `[[链接]]`，并移除指向你已删除记忆的引用。

重写被标记的笔记需要模型。“自动”模式下，文澜在后台完成；“随叫”模式下，AI 工具会得知哪些笔记在等待更新，并能在工作时顺手更新。

[Steep 指南：触发条件、审核规则与模型设置（英文）](docs/steep.md)。可选择设备端模型、本地服务器或云端提供商；由模型驱动的后台工作为选配。

<a id="code-repository-wiki"></a>

<br>

## 为代码仓库建 wiki

在代码仓库中运行 `/wenlan wiki`（Codex 中为 `$wenlan wiki`）。AI 工具会依据仓库本身规划 wiki：先写一篇列出所有笔记的总览，再为新人需要了解的每个部分或流程各写一篇。小工具只需几篇，大型 monorepo 会多得多。再次运行时只补写缺少的笔记。

- **与代码绑定：** 每篇笔记都会记录它所依据的代码行。代码改动时，笔记会显示改了什么，并提供一个审核按钮；沉淀会把笔记更新到最新。
- **以 pull request 放进仓库：** Space 上的 GitHub 按钮会把 Space 连接到对应的仓库，并把依据这些代码的笔记以 pull request 放进仓库的 `wiki/` 文件夹。可以从按钮运行，也可以开启每日更新：在你的电脑上用你自己的 AI 工具运行，或在 GitHub Actions 上运行（AI 工具的密钥存为仓库 secret），电脑关着也可以。同一个 pull request 还会修正仓库自身文档中与代码不再相符的句子，供你审核。
- **保留你的修改：** 有人在仓库中修改过的 wiki 笔记会保留那次修改。引用你的记忆或对话的笔记不会放进仓库。

<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>
<a id="这是什么"></a>
<a id="what-is-this"></a>
<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="knowledge-graph"></a>

<a id="浏览和整理个人知识"></a>

<a id="让知识彼此连接"></a>

<br>

## 把散落的资料整理成知识

<a id="探索你的个人知识图谱"></a>

<p align="center">
  <img src="./docs/assets/wenlan-personal-graph-zh-Hans.png" alt="Wenlan 实机知识图谱，搭配个人兴趣、研究与开发的区域示意" width="100%">
</p>
<p>保留实机节点与连线；文字经翻译，区域名称与范围为示意。</p>

**来源**保留文档与对话；**记忆**记录值得重用的决定、经验与理由。将两者整理成可编辑的**带引用 Markdown 笔记**，用 `[[wikilinks]]` 连接笔记，并沿引用回到原始资料。笔记可以包含 Mermaid 图表，由 App 绘制；AI 工具写入笔记时，文澜会检查图表的常见错误，并指出要修改的那一行。

- **原子记忆：** 遵循 [Zettelkasten 的原子性原则](https://zettelkasten.de/posts/create-zettel-from-reading-notes/)，让每条发现都能独立检索和重用，无需加载整段对话或笔记。
- **共享主题：** 配置整理模型后，LLM 实体提取与实体消歧会将文档和对话中对同一人物、项目或概念的提及关联起来，并保留其记忆与来源。
- **知识群集：** 将相连主题分组，发现资料中的相关知识。默认使用 Label Propagation；Leiden 可作为选配算法。[图谱详情](docs/technical-foundations.md#graph-data-and-entity-resolution)。

<a id="daily-workflow"></a>

<br>

## 日常使用

工作中留下新发现与进度，下次直接接着用。

<p align="center">
  <img src="./docs/assets/wenlan-daily-workflow-zh-Hans.png" alt="来源与已有记忆积累在 Wenlan；在 App 或 AI 中使用，再存回新记忆与工作进度，供下次继续。" width="100%">
</p>

简化流程示意；插件指令与笔记整理均按需使用，没有固定顺序。

- **在 App 中使用。** 添加资料、撰写或编辑笔记、查看引用来源与审核修订，无需连接 AI。点菜单栏（或系统托盘）图标会打开一个小面板，列出需要你处理的事、最近的笔记和快速记下。
- **运用知识、编写新笔记。** 请 AI 找回相关来源与记忆，用于当前任务（`/recall`）；也能编写带引用的新笔记，或依据现有版本更新笔记，再存回 Wenlan。笔记用 `[n]` 引用来源时，文澜会拿每个引用的句子对照它的来源；引用大多对不上时，不会保存该笔记。
- **留下你的新发现。** 保存决定或心得，以及其理由和来源；可在 App 操作，或请 AI 代存（`/capture`）。
- **告一段落，下次继续。** 保存发现与会话记录，更新受影响的笔记，并将进度和下一步写进项目摘要 Brief（`/handoff`）。下次读取这些上下文继续工作（`/brief`）。

**按需整理积累的记忆：** 将相关记忆整理或更新为带引用的笔记（`/wenlan distill`），不是编写新笔记的必要步骤。[插件指令](plugin/skills/README.md)。

<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

<a id="what-wenlan-is-not"></a>
<a id="retrieval"></a>
<a id="从关键词语义与关联找回正确内容"></a>
<a id="retrieval-across-words-meaning-and-connections"></a>

<a id="用-ai-延续已有知识"></a>

<a id="让-ai-接续已有知识"></a>

<br>

## 让知识活用起来

资料放进文澜后，AI 就能接着用：找回带有依据的既有决定、保存新成果，并随着证据变化继续使用。

<a id="if-you-already-use-claude-code-on-your-notes"></a>

### 如果你已经用 Claude Code 或 Codex 管笔记

继续用就好。文澜接上同一个 AI 工具，补上一个 Markdown 文件夹自己做不到的检查：引用大多和来源对不上的笔记不会保存，来源变了会标记，AI 改你写的笔记先等你审核，而且没有你同意，不会改动你原有的笔记。

**适用场景：**

- **开发：** 将先前的技术决定、经验与验收标准带入实现和调试。
- **产品与网站：** 运用客户研究、定位与设计原则撰写规格或构建网页。
- **研究与学习：** 将阅读资料与保存的发现整理成带引用的报告或可复用的学习资料。

**检索与连接：** 插件会引导检索和保存；MCP 连接则提供工具供 AI 调用。

- **[本地混合检索（Hybrid Retrieval）](docs/technical-foundations.md#retrieval-pipeline)**：用 SQLite FTS5 找出相同词句、BGE 向量找出相近语义（libSQL 的 DiskANN 向量索引），再以加权 RRF 合并排名；基础搜索无需 API key。
- **图谱辅助排序：** 共享实体链接可提升相关记忆的排名，即使措辞不同也能关联。可选的 cross-encoder 重排会比较每条候选内容与问题，以调整顺序。
- **按 Space 限定检索：** 选择 Space，将返回的知识限制在相关项目或个人资料集合内。

在“设置”中开启“网页访问”开关，即可让 Claude、ChatGPT 或 Muse 等受支持的网页 AI 客户端，在你的电脑保持联网时搜索整个资料库或指定空间。允许应用时可以选“只能搜索”或“搜索和写入”；可以写入的应用还能新增、修改和删除内容，AI 应用可以在每次改动前先问你。此功能仍处于实验阶段；查询与结果会经过中继服务，详见[隐私边界](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector)。

<a id="architecture"></a>

<br>

## 系统架构

**共用本地知识库。** App、CLI 与 AI 工具通过 Wenlan 本地服务读写。笔记以 Markdown 保存；记忆、关系与搜索索引由本地 libSQL 管理。

**技术：** 本地服务、CLI 和 MCP server 用 Rust 写成；桌面 app 用 Tauri，界面是 React 和 TypeScript。

<p align="center">
  <img src="./docs/assets/wenlan-architecture-zh-Hans.png" alt="架构示意：App 与 AI 工具共用本地知识库，后台整理模型为可选项。" width="100%">
</p>

架构示意。[检索、引用与模型](docs/technical-foundations.md)。

<a id="models-and-privacy"></a>

<br>

## 数据与隐私

**导出知识库：** 将符合条件的笔记导出为 Open Knowledge Format（OKF）知识库。

云端 AI 客户端可能会把找回的知识传给其服务商，云端模型也会收到它处理所需的内容。本地存储不代表这些交互都留在本机。使用统计默认关闭。

[文件位置、备份与卸载](docs/setup-and-data.zh-Hans.md#backup-and-removal) · [联网与隐私详情](docs/PRIVACY.md#when-wenlan-reaches-the-network)。

<a id="evaluation"></a>

<br>

## 评估

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
