<!-- README_SYNC: source=README.md sha256=c577ff92a2d459e1aa449f29ab8148cf794573f2e50ed6a12c08c1bd2b5b100b -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-zh-Hans-mobile.png">
    <img src="./docs/assets/readme-banner-zh-Hans.png" alt="Wenlan: 持续更新的个人知识库" width="100%">
  </picture>
</p>

<p align="center">
  文澜将散落的笔记、文档与对话，整理成持续更新、来源可查的知识库，<br>
  让你和 Claude、Codex 等 AI 工具用上之前积累的知识，不必反复解释过去的决定。
</p>

<p align="center">
  <a href="./README.md">English</a> | 简体中文 | <a href="./README.zh-Hant.md">繁體中文</a> | <a href="./README.es-ES.md">Español</a>
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="最新版本" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?label=release"></a>
  <a href="#license"><img alt="许可证：Apache-2.0 与 AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<p align="center">
  <a href="#start-in-30-seconds">开&#8288;始&#8288;使&#8288;用</a> ·
  <a href="#what-can-i-bring-in">功&#8288;能</a> ·
  <a href="#learn-more">文&#8288;档</a> ·
  <a href="https://discord.gg/nmzVTTU3HG">Discord</a>
</p>

<!-- R113 walkthrough: approved 0.5-second cover; 1080p; original video/audio and masters retained. -->

https://github.com/user-attachments/assets/60b791b0-9726-4fbc-8f08-7e1ab0f5d89d

<p>封面为实机画面与示范数据的翻译合成图；图谱为示意，非 App 原生三栏版面。</p>

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>
<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>
<a id="claude-code-in-30-seconds"></a>
<a id="codex-plugin"></a>
<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

## 开始使用

把资料保存在 Wenlan，让你常用的 AI 用上这些知识，回答问题、完成工作。

### 1. 下载并打开 Wenlan

[下载桌面版](https://github.com/wenlan-app/wenlan/releases/latest)，安装后打开：

- **macOS（Apple Silicon）：** 打开 `.dmg`，把 Wenlan 拖进“应用程序”。App 已签名并通过公证。
- **Windows x64：** 运行 `-setup.exe`，安装包尚未签名。若 SmartScreen 警告，请先确认文件来自官方 Releases 页面，再选“更多信息”→“仍要运行”。
- **Linux：** 暂无桌面版；请按[AI 设置指南](docs/setup-with-ai.md#install-the-runtime)安装 runtime。

首次启动会下载本地搜索模型，设置完成前请保持联网。[下载与隐私说明](docs/PRIVACY.md#when-wenlan-reaches-the-network)。

也可[不安装桌面应用](docs/setup-and-data.zh-Hans.md#installation)，通过 AI 或 CLI 使用同一个本地知识库。

### 2. 保存你的第一个页面

选择“开始使用”进入 Wenlan，创建或导入一个页面并保存。也可以先看看欢迎页中的示例。

### 3. 需要时再连接 AI 工具

| AI 工具 | 连接方式 | 设置入口 |
|---|---|---|
| Claude Code | 插件 | [Claude Code 指南](docs/setup-with-ai.md#claude-code) |
| Codex | 插件 | [Codex 指南](docs/setup-with-ai.md#codex) |
| Cursor、Claude Desktop、VS Code、Gemini CLI、Muse Code | MCP | [MCP 指南](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |

安装插件后，开启新的 AI 对话并运行 `/setup`。连接 AI 无需另装本地语言模型。

连接后，就可以问 AI：

> 用 Wenlan 里的“页面标题”，帮我完成这个任务：……

<a id="daily-workflow"></a>

## 日常使用

你可以单独使用 App，也可以搭配 AI 工作。把值得留下的知识与进度存回 Wenlan，下次接着用。

<p align="center">
  <img src="./docs/assets/wenlan-daily-workflow-zh-Hans.png" alt="来源与已有记忆积累在 Wenlan；在 App 或 AI 中使用，再存回新记忆与工作进度，供下次继续。" width="100%">
</p>

简化流程示意；插件指令与笔记整理均按需使用，没有固定顺序。

- **从你的资料开始。** 导入文档和选定的对话作为来源，或在 App 编写笔记。你也能编辑内容、查看引用来源、审核修订，无需连接 AI。
- **运用知识、编写新笔记。** 请 AI 用 `/recall` 找回相关来源与记忆，运用于任务中；也能编写带引用的新笔记，或依据现有版本更新笔记，再存回 Wenlan。
- **留下你的新发现。** 在 App 中，或请 AI 使用 `/capture`，保存决定或心得，以及其理由和来源。
- **告一段落，下次继续。** 用 `/handoff` 保存记忆、会话记录和下一步；如果新发现改变了本次用到的笔记，也会一并更新。下次用 `/brief` 读取项目摘要，接回这些上下文。

**按需整理积累的记忆：** `/distill` 将相关记忆整理或更新为带引用的笔记，不是编写新笔记的必要步骤。[插件指令](plugin/skills/README.md)。

<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>
<a id="这是什么"></a>
<a id="what-is-this"></a>
<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="what-can-i-bring-in"></a>
<a id="knowledge-graph"></a>

<a id="浏览和整理个人知识"></a>

<a id="让知识彼此连接"></a>

## 把散落的资料整理成知识

**来源**保留你读过的资料；**记忆**留下你的发现与判断理由。将两者整理成**带有引用的 Markdown 笔记**，供你阅读、编辑，用 `[[wikilinks]]` 连接相关内容，也能通过引用查回原始资料。

- **文件与对话导入**：导入 Markdown、文本文件、文本型 PDF、文件夹，以及 ChatGPT 或 Claude 导出的 ZIP，保留来源引用。扫描版 PDF 需先提取文本再导入。对话导入保留当时记录；已注册的文件来源可刷新为最新内容。
- **Obsidian 只读连接**：读取已有仓库并按需刷新，不修改原始文件，也不将变化同步回去。
- **原子记忆**：单独找回一条发现，不必加载整段对话或整篇笔记。
- **知识图谱**：通过不同类型的实体、关系与知识群集（communities），发现相关主题；配置整理模型后，将重复提及连接到同一实体。

<a id="探索你的个人知识图谱"></a>

<p align="center">
  <img src="./docs/assets/wenlan-personal-graph-zh-Hans.png" alt="Wenlan 实机知识图谱，搭配个人兴趣、研究与开发的区域示意" width="100%">
</p>
<p>保留实机节点与连线；文字经翻译，区域名称与范围为示意。</p>

<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

<a id="what-wenlan-is-not"></a>
<a id="retrieval"></a>
<a id="从关键词语义与关联找回正确内容"></a>
<a id="retrieval-across-words-meaning-and-connections"></a>

<a id="用-ai-延续已有知识"></a>

## 让 AI 接续已有知识

在不同 AI 工具间共用同一个知识库。**插件**引导 AI 何时检索先前上下文、保存新决定；**仅连接 MCP** 则让 AI 可按需或根据你的要求调用这些工具。

- **[本地混合检索（Hybrid Retrieval）](docs/technical-foundations.md#retrieval-pipeline)**：用 SQLite FTS5 找出相同词句、BGE 向量找出相近语义，再以加权 RRF 合并排名；基础搜索无需 API key。
- **图谱辅助排序**：有可用的实体连接时，提高相关记忆的排名，也可选用 cross-encoder reranking 精排结果。
- **Spaces 范围检索**：选定 Space，将返回的知识限制在相关项目或个人资料集合内。

<p align="center">
  <img src="./docs/assets/wenlan-knowledge-to-work-zh-Hans.png" alt="AI 接着做，不必从头交代：关键词、语义与图谱检索" width="100%">
</p>
<p>虚构案例，非产品测试；文澜补充上下文，与 Claude 原生记忆配合使用。</p>

在“设置”中开启“网页访问”开关，即可让 Claude、ChatGPT 或 Muse 等受支持的网页 AI 客户端，在你的电脑保持联网时搜索整个资料库或指定空间。此功能仍处于实验阶段；查询与结果会经过中继服务，详见[隐私边界](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector)。

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

## 更新知识，保留历史

**依据引用关系更新。** Wenlan 中的来源或记忆变更、移除时，引用它的笔记会被标记为待更新。启用后台 AI 并有可用模型后，符合条件的笔记会根据最新资料重新生成；引用不足的草稿不会采用。

- **Steep 后台整理**：为已保存的记忆补上类型、结构化信息、搜索线索与图谱连接；也可选择将记忆与来源合成为带有引用的笔记。
- **[修订审核与历史](docs/knowledge-guide.zh-Hans.md#updates-and-history)**：查看笔记由谁修改、比较版本并追溯记忆的替代关系；后台更新会为你创建或编辑过的笔记提出修订，而设为需审核的笔记也会拦下 AI 主动修改，等待你确认。
- **维基导出**：将符合条件的笔记导出为 Open Knowledge Format（OKF）维基。
- **[可选的后台模型](docs/setup-and-data.zh-Hans.md#models)**：用设备端 Qwen（llama.cpp）、Ollama 或 LM Studio 等 OpenAI 兼容本地服务，或已配置的云端提供商启用整理、笔记合成和自动更新。

<p align="center">
  <img src="./docs/assets/wenlan-updates-history-zh-Hans.png" alt="示意流程：文件来源中的发现将记忆由 V1 更新为 V2，再影响自动更新或待审修订的笔记。" width="100%">
</p>
<p>示意图依据当前的更新行为绘制；并非 App 截图。</p>

<a id="architecture"></a>

## 系统架构

**共用本地知识库。** App、CLI 与 AI 工具通过 Wenlan 本地服务读写。笔记以 Markdown 保存；记忆、关系与搜索索引由本地 libSQL 管理。

<p align="center">
  <img src="./docs/assets/wenlan-architecture-zh-Hans.png" alt="架构示意：App 与 AI 工具共用本地知识库，后台整理模型为可选项。" width="100%">
</p>

架构示意。[检索、引用与模型](docs/technical-foundations.md)。

<a id="models-and-privacy"></a>

## 数据与隐私

云端 AI 客户端可能会把找回的知识传给其服务商，云端模型也会收到它处理所需的内容。本地存储不代表这些交互都留在本机。使用统计默认关闭。

[文件位置、备份与卸载](docs/setup-and-data.zh-Hans.md#backup-and-removal) · [联网与隐私详情](docs/PRIVACY.md#when-wenlan-reaches-the-network)。

<a id="evaluation"></a>

## 评估

这些 LongMemEval（LME）测量结果评估的是已存知识的检索，不是 AI 回答质量。Recall@5 表示相关资料出现在前五条结果中的比例。

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->

[评估方法与基准详情](docs/eval/README.md)。

<a id="learn-more"></a>
<a id="离线队列outbox"></a>

## 进一步了解

- [开始使用与安装](docs/setup-and-data.zh-Hans.md#installation)
- [连接 AI 工具](docs/setup-with-ai.md)
- [来源与导入](docs/knowledge-guide.zh-Hans.md#sources-and-pages) · [本地文件、备份与离线使用](docs/setup-and-data.zh-Hans.md)
- [隐私](docs/PRIVACY.md)
- [Doctor 与问题反馈](https://wenlan.app/docs/diagnostics-and-issue-reports)
- [完整文档索引](docs/README.md)

## 贡献

欢迎加入 [Discord 社区](https://discord.gg/nmzVTTU3HG)。请在 [GitHub issues](https://github.com/wenlan-app/wenlan/issues) 反馈问题或提出功能建议；Issue 公开可见，分享前请遮盖密钥和个人资料。代码与文档贡献请见[贡献指南](.github/CONTRIBUTING.md)、[安全问题反馈](.github/SECURITY.md)和[行为准则](.github/CODE_OF_CONDUCT.md)。

<a id="code-signing-policy"></a>

## 代码签名政策

请见[各平台签名与公证说明](docs/code-signing.md)。

<a id="license"></a>

## 许可

本地 runtime、CLI、MCP server、shared types 和 plugins 使用 [Apache-2.0](LICENSE)。桌面 app 及其前端使用 [AGPL-3.0-only](app/LICENSE)；修改版若作为网络服务运行，须向用户提供其源代码。

<a id="acknowledgments"></a>

## 源流与同类项目

Wenlan 名称来自文澜阁，产品方向承接 LLM-wiki 与 agent-memory 脉络：[Karpathy 的 LLM-wiki note](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)、[Rohitg00 的提案](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2)和 [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki)。
