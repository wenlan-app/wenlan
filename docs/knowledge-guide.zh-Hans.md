# 知识指南：来源、记忆、页面与搜索

本指南说明 Wenlan 如何连接来源材料、记忆、页面、图谱关系、搜索、更新、审核与本地文件。

语言：[English](knowledge-guide.md) · [繁體中文](knowledge-guide.zh-Hant.md) · [简体中文](knowledge-guide.zh-Hans.md) · [Español](knowledge-guide.es-ES.md)<br>
[返回简体中文 README](../README.zh-Hans.md)

目录：[来源与页面](#sources-and-pages) · [图谱与搜索](#graph-and-search) · [更新与历史](#updates-and-history)

<a id="sources-and-pages"></a>

## 来源与页面

README 中的示例取材自[Agent 设计指南](https://www.anthropic.com/engineering/building-effective-agents)与[这份 README 的图稿审查修正](https://github.com/wenlan-app/wenlan/commit/061fbc6ab12a76ec805869e46d464a58a06db296)，示范如何留下可复用的工作准则，不是用户成效数据或自动生成页面的实测记录。示例插图请见 README 的[示意图](../README.zh-Hans.md#what-does-wenlan-build)。

Wenlan 让持续进行的工作不只留在聊天窗口里。你可以保存选定的文档和对话，记下过程中的决定，再把它们整理成可阅读、编辑和重用的页面。你可以直接请已连接的 AI 撰写页面；可选的模型支持合成与后台维护则需要[配置模型](setup-and-data.zh-Hans.md#models)。

<a id="what-wenlan-is-not"></a>

**适合需要长期延续的工作。** 如果你连续几天或几周用 AI 处理同一个主题，却常常要翻找之前的资料、重新解释已经做出的决定，Wenlan 就是为这种工作流程设计的。它不是生活管理系统，也不是嵌入其他产品的 memory SDK。你仍可继续使用 Obsidian；Wenlan 不承诺替代它的插件或迁移 vault 的所有功能。

**一个知识系统，三种角色：**

- **来源让 Wenlan 读到的材料始终可追溯。** 导入的对话保留为捕获时的记录；已注册文件会随内容变化同步当前版本。
- **记忆保留工作真正教会你的内容。** AI agent 捕获原子的决策、经验、修正与取代关系，并保留出处。
- **页面汇总当前知识。** Wenlan 把相关来源与记忆整理成带引用的 Markdown，让你反复使用、刷新与审核。

**更新方式：** 来源与捕获的记忆都可以作为同一页面的依据。记忆历史记录各项记忆的变化；页面历史则记录支撑页面的依据与修订。自动更新时，符合条件、由系统维护的页面可以直接更新；你编辑过的页面则会收到修订提案。审核让你决定是否应用更新，但不代表 AI 的结论一定正确。

技术读者可参考：Wenlan 遵循 **LLM wiki** 模式。数据模型、检索与维护规则请见 [LLM-wiki 实现指南](https://wenlan.app/zh-CN/learn/distilled-wiki-pages-ai-memory) 和[技术基础](technical-foundations.md)。

<a id="graph-and-search"></a>

## 图谱与搜索

Agent Loop 不只是一个页面：它连接重试准则、界面检查的经验，以及可继续使用的验收清单。

实体关系图谱只是 Wenlan 一大套互相链接的页面的一部分。**知识页面**保留持续维护的结论，**实体**固定可复用的人物、项目与概念，**来源页面**让导入或同步的材料可检查，原子**记忆**则保留决策与变化。它们通过彼此分开的明确连接协作：页面间的 wikilink、页面依据、记忆到实体的连接，以及实体间的有向关系。

在实体图谱这一层，配置 enrichment 模型后，Wenlan 会从记忆中提取带类型的实体、观察与有方向的关系。实体链接与解析会复用已有节点，而不是把每次提及都当成新事物；每条记忆仍保留来源，并可连接多个实体。[查看连接模型如何存储 ->](technical-foundations.md#connected-knowledge-model)

- **含义与方向：** 关系使用 `uses`、`part_of`、`contradicts`、`replaced_by` 等预置词汇；未知类型会回退为 `related_to`，并成为可审核的词汇提案。
- **强度与出处：** 关系可以保存置信度、解释与对应的来源记忆，让强弱不同的主张仍可区分、可检查。
- **形成可复用群组：** 标签传播会依关系密度为实体分组，并按每对实体之间的关系数量加权。这些群组可组织可选的全局摘要，实体链接也会为检索补充脉络。
- **修正但不抹除：** 相关说法、修正与明确的取代关系可以放在一起检查，原始来源与记忆历史仍会保留。

检索时，Wenlan 会用实体向量匹配找到与问题相关的实体。存在符合条件的图谱链接时，默认开启的图谱记忆信号会把相连记忆作为第三路 [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf) 排名信号加以提升。这个路径取决于现有图谱数据与读取范围，空间边界仍然有效。[查看图谱检索如何工作 ->](technical-foundations.md#graph-assisted-retrieval)

<a id="retrieval"></a>

### 从关键词、语义与关联找回正确内容

Wenlan 的核心搜索是本地混合检索流程，不是单一的向量查询。每个阶段负责不同工作：

- **原词匹配，[SQLite FTS5](https://www.sqlite.org/fts5.html)：** 全文索引查找字面关键词、标识符与短语。
- **相近含义，FastEmbed + [`Qdrant/bge-base-en-v1.5-onnx-Q`](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q)：** 量化的英文模型会产生 768 维语义向量；[libSQL cosine DiskANN](https://turso.tech/blog/approximate-nearest-neighbor-search-with-diskann-in-libsql) 再以近似最近邻搜索（ANN）取得候选。
- **合并排名，加权 [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf)（`k = 60`）：** 融合原词与语义排名，不假设两者的原始分数采用同一尺度；向量信号还会由余弦相似度加权。
- **关联脉络，图谱记忆信号：** 符合条件的实体链接会加入第三路 RRF 信号，返回的记忆仍受当前读取范围限制。
- **可选精排，交叉编码器（cross-encoder）：** 与分别编码查询和记忆的 embedding 不同，[`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en) 或 [`BAAI/bge-reranker-base`](https://huggingface.co/BAAI/bge-reranker-base) 会同时读取查询与单个候选，再对较小的候选池重新排名；默认关闭。

页面、情节记忆与事实（fact）通道都需要主动启用；不可用时会退回其余搜索信号。空间仍负责限制读取范围。[查看方法、默认值与限制 ->](technical-foundations.md)

<a id="updates-and-history"></a>

## 更新与历史

一次生成的页面会过时；只存记忆又容易碎成互不相连的事实。Wenlan 连接两套生命周期，但不把它们混成同一层。

### 原子记忆

`CAPTURE -> CLASSIFY -> ENRICH -> LINK -> RECONCILE`

Capture 与明确的 supersession 属于核心流程。模型支持的阶段只会在配置相应模型后运行，Reconcile 默认关闭。

| 操作 | Wenlan 做什么 |
|---|---|
| **Capture** | AI agent 每次写入一条完整、自足的想法，遵循 Zettelkasten 的原子笔记原则，而不是保存整段对话。 |
| **Classify** | 配置语言模型后，Wenlan 将记忆分为 `identity`、`preference`、`decision`、`lesson`、`gotcha` 或 `fact`；调用方明确提供的准确类型优先。 |
| **Enrich** | 配置语言模型后，在可用时补充结构化字段、检索提示、事件日期、质量、重要性与标签。 |
| **Link** | 保留出处；启用 enrichment 后，把记忆连接到知识图谱中的实体与关系。 |
| **Reconcile** | 明确取代旧说法时保留 `supersedes` 链。若发起替换的 agent 信任级别低于 full，该替换会自动进入人工审核队列，无需任何开关。可选的模型流程还可以把受保护内容的冲突放入审核，而不是覆盖历史；这个流程默认关闭，必须明确启用。 |

高级设置：使用 `WENLAN_ENABLE_DUAL_POOL_RESOLVE=1` 启用这个 Reconcile 流程。

### 持续维护的页面

`DISTILL -> CITE -> TRACK -> REFRESH -> REVIEW`

| 操作 | Wenlan 做什么 |
|---|---|
| **Distill** | 把相关来源与记忆汇总成一个 Markdown 页面。 |
| **Cite** | 保留引用记录与验证状态；自动 refresh 若未通过引用支撑检查，就会丢弃草稿。 |
| **Track** | 记录哪些证据支撑页面、页面为何过时，以及有上限的变更记录。 |
| **Refresh** | 页面被标记为过时后，依当前证据重建符合条件、由机器维护的页面。 |
| **Review** | 自动更新时，对你编辑过的页面提出修订，而非静默改写。 |

例如，导入一份设计文档，再让 Codex 记下一项调试决策。Wenlan 可以把两者整理成同一个页面，并引用两者。自动更新时，页面会依据当前材料重建；如果你编辑过它，更新提案会等你审核。

**审核范围：** 这是一项更新政策，不是保护你文件的安全机制。直接编辑文件或通过本地手动编辑 API 所做的修改，不会进入这个审核队列。明确强制重新生成也可能替换已编辑的页面；桌面 app 会在执行前要求确认。

<a id="local-markdown"></a>

### 与 Obsidian 共存的本地 Markdown

长期知识保留为普通文件，不被锁在专有编辑器格式里：

- **纯文本文件：** 页面与 session notes 都以 Markdown 保存在 `~/.wenlan/`。
- **可检查的历史：** 每一页都保留修订记录（app 里可看，MCP 有 `get_page_revisions`）；而且全部都是纯文本文件，想要的话可以自己把 `~/.wenlan` 纳入版本控制。
- **与 Obsidian 共存：** Wenlan 把现有 vault 当作来源读取。你可以把 `~/.wenlan/pages/` symlink 到 vault，或从桌面 app 导出页面；你的编辑仍由你拥有，之后的机器更新会成为可审核的修订建议。

不开 app 也能读页面：

```text
$ wenlan pages "embedding work" --print
```
