# 知識指南：來源、記憶、頁面與搜尋

本指南說明 Wenlan 如何串連來源材料、記憶、頁面、圖譜關係、搜尋、更新、審核與本地檔案。

語言：[English](knowledge-guide.md) · [繁體中文](knowledge-guide.zh-Hant.md) · [简体中文](knowledge-guide.zh-Hans.md) · [Español](knowledge-guide.es-ES.md)<br>
[回到繁體中文 README](../README.zh-Hant.md)

內容：[來源與頁面](#sources-and-pages) · [圖譜與搜尋](#graph-and-search) · [更新與歷史](#updates-and-history)

<a id="sources-and-pages"></a>

## 來源與頁面

README 中的示例取材自[Agent 設計指南](https://www.anthropic.com/engineering/building-effective-agents)與[這份 README 的圖稿審查修正](https://github.com/wenlan-app/wenlan/commit/061fbc6ab12a76ec805869e46d464a58a06db296)，示範如何留下可重用的工作準則，不是使用者成效數據或自動產頁的實測紀錄。示例插圖請見 README 的[示意圖](../README.zh-Hant.md#what-does-wenlan-build)。

Wenlan 讓持續進行的工作不只留在聊天視窗裡。你可以保存選定的文件與對話，記下過程中的決定，再把它們整理成可閱讀、編輯和重用的頁面。你可以直接請已連接的 AI 撰寫頁面；選用的模型支援合成與背景維護則需要[設定模型](setup-and-data.zh-Hant.md#models)。

<a id="what-wenlan-is-not"></a>

**適合需要長期延續的工作。** 如果你會連續幾天或幾週用 AI 處理同一個主題，卻常常要翻找先前資料、重新解釋已做的決定，Wenlan 就是為這種工作流程設計的。它不是生活管理系統，也不是嵌入其他產品的 memory SDK。你仍可繼續使用 Obsidian；Wenlan 不承諾取代其外掛或完整搬移 vault 功能。

**一個知識系統，三種角色：**

- **來源讓 Wenlan 讀到的材料始終可追溯。** 匯入的對話保留為擷取時的紀錄；已登錄檔案會隨內容變化同步目前版本。
- **記憶保留工作真正教會你的內容。** AI agent 擷取原子的決策、經驗、修正與取代關係，並保留出處。
- **頁面彙整目前知識。** Wenlan 把相關來源與記憶整理成附有引用的 Markdown，讓你反覆使用、更新與審核。

**更新方式：** 來源與記下的記憶都能作為同一頁面的依據。記憶歷史會記錄個別記憶的變化；頁面歷史則記錄支撐頁面的依據與修訂。自動更新時，符合條件、由系統維護的頁面可直接更新；你編輯過的頁面則會收到修訂提案。審核讓你決定是否套用更新，但不代表 AI 的結論一定正確。

技術讀者可參考：Wenlan 採用 **LLM wiki** 的做法。資料模型、檢索與維護規則詳見 [LLM-wiki 實作指南](https://wenlan.app/zh-TW/learn/distilled-wiki-pages-ai-memory) 與[技術基礎](technical-foundations.md)。

<a id="graph-and-search"></a>

## 圖譜與搜尋

Agent Loop 不只是一個頁面：它連著重試準則、介面檢查的經驗，以及可接著使用的驗收清單。

實體關係圖譜只是 Wenlan 一大套互相連結的頁面的一部分。**知識頁面**保留持續維護的結論，**實體**固定可複用的人物、專案與概念，**來源頁面**讓匯入或同步的材料可檢查，原子**記憶**則保留決策與變化。它們透過彼此分開的明確連接協作：頁面間的 wikilink、頁面依據、記憶到實體的連接，以及實體間的有向關係。

在實體圖譜這一層，設定 enrichment 模型後，Wenlan 會從記憶中提取帶有類型的實體、觀察與有方向的關係。實體連結與解析會複用既有節點，而不是把每次提及都當成新事物；每條記憶仍保留來源，並可連結多個實體。[查看連接模型如何儲存 ->](technical-foundations.md#connected-knowledge-model)

- **含義與方向：** 關係使用 `uses`、`part_of`、`contradicts`、`replaced_by` 等預置詞彙；未知類型會回退為 `related_to`，並成為可審核的詞彙提案。
- **強度與出處：** 關係可以保存信賴度、解釋與對應的來源記憶，讓強弱不同的主張仍可區分、可檢查。
- **形成可複用群組：** 標籤傳播會依關係密度為實體分組，並按每對實體之間的關係數量加權。這些群組可組織選用的全域摘要，實體連結也會為檢索補充脈絡。
- **修正但不抹除：** 相關說法、修正與明確的取代關係可以放在一起檢查，原始來源與記憶歷史仍會保留。

檢索時，Wenlan 會用實體向量比對找到與問題相關的實體。存在符合條件的圖譜連結時，預設開啟的圖譜記憶訊號會把相連記憶作為第三路 [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf) 排名訊號加以提升。這個路徑取決於現有圖譜資料與讀取範圍，空間邊界仍然有效。[查看圖譜檢索如何運作 ->](technical-foundations.md#graph-assisted-retrieval)

<a id="retrieval"></a>

### 從關鍵字、語意與關聯找回正確內容

Wenlan 的核心搜尋是本地混合檢索流程，不是單一的向量查詢。每個階段負責不同工作：

- **原詞比對，[SQLite FTS5](https://www.sqlite.org/fts5.html)：** 全文索引查找字面關鍵字、識別碼與短語。
- **相近含義，FastEmbed + [`Qdrant/bge-base-en-v1.5-onnx-Q`](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q)：** 量化的英文模型會產生 768 維語意向量；[libSQL cosine DiskANN](https://turso.tech/blog/approximate-nearest-neighbor-search-with-diskann-in-libsql) 再以近似最近鄰搜尋（ANN）取得候選。
- **合併排名，加權 [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf)（`k = 60`）：** 融合原詞與語意排名，不假設兩者的原始分數採用同一尺度；向量訊號還會由餘弦相似度加權。
- **關聯脈絡，圖譜記憶訊號：** 符合條件的實體連結會加入第三路 RRF 訊號，傳回的記憶仍受目前讀取範圍限制。
- **可選精排，交叉編碼器（cross-encoder）：** 與分別編碼查詢和記憶的 embedding 不同，[`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en) 或 [`BAAI/bge-reranker-base`](https://huggingface.co/BAAI/bge-reranker-base) 會同時讀取查詢與單一候選，再對較小的候選池重新排名；預設關閉。

頁面、情節記憶與事實（fact）通道都需要主動啟用；不可用時會退回其餘搜尋訊號。空間仍負責限制讀取範圍。[查看方法、預設值與限制 ->](technical-foundations.md)

<a id="updates-and-history"></a>

## 更新與歷史

一次生成的頁面會過時；只存記憶又容易碎成互不相連的事實。Wenlan 連結兩套生命週期，但不把它們混成同一層。

### 原子記憶

`CAPTURE -> CLASSIFY -> ENRICH -> LINK -> RECONCILE`

Capture 與明確的 supersession 屬於核心流程。模型支援的階段只會在設定相應模型後執行，Reconcile 預設關閉。

| 操作 | Wenlan 做什麼 |
|---|---|
| **Capture** | AI agent 每次寫入一條完整、自足的想法，遵循 Zettelkasten 的原子筆記原則，而不是保存整段對話。 |
| **Classify** | 設定語言模型後，Wenlan 將記憶分為 `identity`、`preference`、`decision`、`lesson`、`gotcha` 或 `fact`；呼叫端明確提供的準確類型優先。 |
| **Enrich** | 設定語言模型後，在可用時補充結構化欄位、檢索提示、事件日期、品質、重要性與標籤。 |
| **Link** | 保留出處；啟用 enrichment 後，把記憶連結到知識圖譜中的實體與關係。 |
| **Reconcile** | 明確取代舊說法時保留 `supersedes` 鏈。若發起替換的 agent 信任等級低於 full，該替換會自動進入人工審核佇列，無需任何開關。可選的模型流程還可以把受保護內容的衝突放入審核，而不是覆蓋歷史；這個流程預設關閉，必須明確啟用。 |

進階設定：使用 `WENLAN_ENABLE_DUAL_POOL_RESOLVE=1` 啟用這個 Reconcile 流程。

### 持續維護的頁面

`DISTILL -> CITE -> TRACK -> REFRESH -> REVIEW`

| 操作 | Wenlan 做什麼 |
|---|---|
| **Distill** | 把相關來源與記憶彙整成一個 Markdown 頁面。 |
| **Cite** | 保留引用紀錄與驗證狀態；自動 refresh 若未通過引用支撐檢查，就會捨棄草稿。 |
| **Track** | 記錄哪些證據支撐頁面、頁面為何過時，以及有上限的變更紀錄。 |
| **Refresh** | 頁面被標記為過時後，依目前證據重建符合條件、由機器維護的頁面。 |
| **Review** | 自動更新時，對你編輯過的頁面提出修訂，而非靜默改寫。 |

例如，匯入一份設計文件，再讓 Codex 記下一項除錯決策。Wenlan 可以把兩者整理成同一個頁面，並引用兩者。自動更新時，頁面會依目前的依據重建；若你編輯過它，更新提案會等你審閱。

**審核範圍：** 這是更新政策，不是保護你檔案的安全機制。直接編輯檔案或透過本機手動編輯 API 所做的修改，不會進入此審核佇列。明確強制重新生成也可能取代已編輯的頁面；桌面 app 會在執行前要求確認。

<a id="local-markdown"></a>

### 與 Obsidian 共存的本地 Markdown

長期知識保留為一般檔案，不被鎖在專有編輯器格式裡：

- **純文字檔案：** 頁面與 session notes 都以 Markdown 保存在 `~/.wenlan/`。
- **可檢查的歷史：** 每一頁都保留修訂紀錄（app 裡可看，MCP 有 `get_page_revisions`）；而且全部都是純文字檔，想要的話可以自己把 `~/.wenlan` 納入版本控制。
- **與 Obsidian 共存：** Wenlan 把現有 vault 當成來源讀取。你可以把 `~/.wenlan/pages/` symlink 到 vault，或從桌面 app 匯出頁面；你的編輯仍由你擁有，之後的機器更新會成為可審核的修訂建議。

不開 app 也能讀頁面：

```text
$ wenlan pages "embedding work" --print
```
