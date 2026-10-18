<!-- README_SYNC: source=README.md sha256=bb09d46a7e82483244078f7bdf6d1a8b0e078d9323b7360707e9443a5260dad1 -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-zh-Hant-mobile.png">
    <img src="./docs/assets/readme-banner-zh-Hant.png" alt="Wenlan：持續更新的個人維基。AI 幫你整理，你保有主導權。" width="100%">
  </picture>
</p>

Wenlan 把你的文件、Markdown 檔案和 AI 對話整理成可編輯、附有來源連結的筆記，讓你和 AI 工具在已有成果上繼續工作。

來源有變，可以請 AI 更新筆記，也能啟用背景更新。如果你編輯過筆記，Wenlan 會提出修訂，讓你確認是否採用，而不是自動覆寫你的內容。

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-Hans.md">简体中文</a> | 繁體中文 | <a href="./README.es-ES.md">Español</a>
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="最新版本" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?sort=semver&label=release"></a>
  <a href="#license"><img alt="授權：Apache-2.0 與 AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<p align="center">
  <a href="#start-in-30-seconds">開&#8288;始&#8288;使&#8288;用</a> ·
  <a href="#what-does-wenlan-build">這&#8288;是&#8288;什&#8288;麼？</a> ·
  <a href="#what-can-it-do">能&#8288;力</a> ·
  <a href="#how-does-it-work">日&#8288;常&#8288;流&#8288;程</a> ·
  <a href="#evaluation">評&#8288;估</a> ·
  <a href="#learn-more">進&#8288;一&#8288;步&#8288;了&#8288;解</a>
</p>

https://github.com/user-attachments/assets/e4c934f9-c284-480f-a20f-708d9c91748d

<p align="center">
  <sub>開場為組合示意，並非 app 的原生三欄介面；後續為 app 實際操作錄影，展示筆記與來源引用。</sub>
</p>

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>

## 開始使用

<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>

### 1. 下載並開啟 Wenlan

[下載桌面版](https://github.com/wenlan-app/wenlan/releases/latest)，安裝後開啟：

- **macOS（Apple Silicon）：** 打開 `.dmg`，把 Wenlan 拖進「應用程式」。App 已簽署並通過公證。
- **Windows x64：** 執行 `-setup.exe`。安裝包尚未簽署。如果 SmartScreen 顯示警告，請先確認檔案來自官方 Releases 頁面，再點選「其他資訊」→「仍要執行」。
- **Linux：** 暫時沒有桌面版；可依照[設定指南](docs/setup-with-ai.md#install-the-runtime)，不透過桌面 app，直接搭配 AI 工具使用。

首次啟動會下載本地搜尋模型，設定完成前請保持連網。[下載與隱私說明](docs/PRIVACY.md#when-wenlan-reaches-the-network)。

<a id="claude-code-in-30-seconds"></a>

<a id="codex-plugin"></a>

<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

### 2. 先寫下自己的筆記

選擇「開始使用」進入 Wenlan，接著寫一篇自己的筆記。歡迎畫面也提供範例，可以先看看。開始寫筆記不必先匯入資料、連接 AI 工具或設定 AI 模型。

### 3. 想帶入來源時再匯入

準備好時，可以[匯入 Markdown 檔案或 ChatGPT／Claude 匯出的對話](#what-can-i-bring-in)。匯入是選用功能；你可以先寫自己的筆記。

### 4. 想使用 AI 時再連接工具

需要 AI 使用同一份知識時，再連接 Claude Code、Codex 等支援的工具。連接後的 AI 可以撰寫筆記，不必另外安裝本地語言模型。

<details>
<summary>連接時需要幫忙？讓 AI 協助設定</summary>

如有提示，請重新啟動 AI 工具。需要協助設定時，把下面這段貼給 Claude Code、Codex，或其他能夠讀取設定指南的工具：

```text
請依照這份指南，將目前的 AI 工具連接到 Wenlan：
https://raw.githubusercontent.com/wenlan-app/wenlan/main/docs/setup-with-ai.md

如果已安裝 Wenlan，請沿用現有安裝。
只設定目前這個 AI 工具，並確認它能儲存及找回一筆測試記憶。
```

</details>

想用已連接的 AI 整理主題時，打開一段值得保留的工作對話，然後提出：

> 把這段對話中［主題］的結論與理由，整理成附來源引用的 Wenlan 筆記。

也可以請 AI 把已匯入資料中的主題整理成筆記。

完成後，在 Wenlan 打開新筆記，點開引用查來源，也能補上自己的想法。下次做相關工作時，請 AI 先讀這篇筆記再繼續。

背景整理與自動更新是可選功能，需要[設定模型](#models-and-privacy)。終端機安裝、其他平台與更新方式，請見[設定指南](docs/setup-and-data.zh-Hant.md#installation)。

遇到問題？[設定指南](docs/setup-with-ai.md) · [回報問題](https://github.com/wenlan-app/wenlan/issues)。Issue 是公開的；分享任何內容（包括紀錄）前，請先遮蔽密鑰、憑證、存取權杖、識別碼，以及私人或個人內容。



<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>

<a id="這是什麼"></a>

<a id="你與-ai-共用的個人維基"></a>

## 你與 AI 共用的個人 Wiki

- **散落的資料，整理成主題。** 文件、Markdown 檔案與 AI 對話，彙整成彼此連結的筆記，引用可以點開查證。
- **自己看，也給 AI 用。** 直接閱讀、編輯，也能讓 Claude Code、Codex 接著用。
- **後續維護有流程，不必每次從頭整理。** 追蹤來源變動、更新相關筆記並保留歷史；你編輯過的內容，先確認修訂再更新。

筆記是本地 Markdown 檔案，你可以閱讀、編輯，也能帶走。

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-system-zh-Hant-mobile.png">
    <img src="./docs/assets/wenlan-system-zh-Hant.png" alt="來源與記憶整理成 Agent Loop 筆記示例：重試原則結合一次具體教訓，測試通過但手機標籤仍被連線遮住。因此把桌面與手機畫面檢查補進驗收條件，供後續使用 Claude 或 Codex 時沿用。" width="100%">
  </picture>
</p>

<a id="what-wenlan-is-not"></a>

圖中是可重用工作準則的整理示例，不代表使用者成效或自動產生筆記的實測。[來源、記憶與筆記如何協作](docs/knowledge-guide.zh-Hant.md#sources-and-pages)。

<a id="knowledge-graph"></a>

<a id="從一頁找到相關的知識"></a>

### 從一篇筆記，找到相關的知識

Agent Loop 不只是一篇筆記：它連著重試準則、介面檢查的經驗，以及可接著使用的驗收清單。

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-knowledge-network-zh-Hant-mobile.png">
    <img src="./docs/assets/wenlan-knowledge-network-zh-Hant.png" alt="Agent Loop 筆記的示意圖，引用 Agent 設計指南與介面檢查紀錄；具名記憶包括重試準則和視覺檢查經驗，並連結到可重用的驗收清單及相關概念與工具。" width="100%">
  </picture>
</p>

<a id="retrieval"></a>
<a id="從關鍵字語意與關聯找回正確內容"></a>

從筆記連到相關概念與依據。[圖譜與搜尋如何運作](docs/knowledge-guide.zh-Hant.md#graph-and-search)。

<a id="what-makes-wenlan-distinct"></a>
<a id="why-is-wenlan-different"></a>
<a id="two-lifecycles"></a>
<a id="two-lifecycles-one-maintained-knowledge-system"></a>
<a id="兩套生命週期一個持續維護的知識系統"></a>

### 知識會改變，歷史仍會保留。

新的經驗補進筆記，過去的判斷仍找得到。以下示例把「測試通過就算完成」改成「測試加上桌面與手機畫面檢查」，保留修正原因與依據。

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-lifecycle-zh-Hant-mobile.png">
    <img src="./docs/assets/wenlan-lifecycle-zh-Hant.png" alt="示意的前後規則：「測試通過就算完成」改為「檢查測試以及桌面與手機畫面」；依據是測試雖通過，手機標籤仍被遮住。自動更新你編輯過的筆記時，修改會以提案交由你審核。" width="100%">
  </picture>
</p>

<a id="local-markdown"></a>
<a id="原子記憶"></a>
<a id="持續維護的頁面"></a>
<a id="與-obsidian-共存的本地-markdown"></a>

審核針對 AI 更新；直接改檔與強制重新整理另有規則。[更新、審核與本地檔案](docs/knowledge-guide.zh-Hant.md#updates-and-history)。

### 已經在用 Obsidian 和 AI 套件？

你可以用 [Copilot](https://docs.obsidiancopilot.com/agent-mode-and-tools/) 讓 AI 讀寫筆記、用 [Smart Connections](https://github.com/brianpetro/obsidian-smart-connections) 找相關內容，再用 [Obsidian Git](https://github.com/Vinzent03/obsidian-git) 保留版本。自由度很高，但套件怎麼搭配、資料何時整理、舊筆記如何更新，仍需要自己安排。

Wenlan 把整理成筆記、追蹤來源變動、更新相關內容和保留修訂歷史接成一套流程，省下自行搭建與維護的工夫。整理好的知識，自己能閱讀，也能讓 Claude Code、Codex 接著用。

可以直接請已連接的 AI 更新筆記，也能[設定模型](#models-and-privacy)啟用背景更新。你編輯過的筆記，更新前先提出修訂，由你確認是否採用。

現有 Obsidian 庫可以接為唯讀來源，不必搬動檔案。Wenlan 不會修改原本的庫，也不會把變更同步回去。


<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="what-can-i-bring-in"></a>

## 能力

### 匯入你的資料

- **留下有用的 AI 對話：** 匯入 ChatGPT 或 Claude 匯出的 ZIP，已匯入的對話不會重複加入。
- **帶入現有檔案：** 匯入 Markdown、文字檔或可擷取文字的 PDF，也能整批讀取資料夾，或把 Obsidian 庫接為來源。掃描版 PDF 須先擷取文字。
- **快速捕捉：** 直接在桌面 app 記下想法或決策，不必先開啟 AI 對話。
- **[請 AI 記住工作重點](https://wenlan.app/learn/ai-work-memory)：** 請 AI 工具記下決策、經驗、更正、偏好與事實，保留來源及取代的舊記錄。
- **帶入其他 wiki：** 透過 CLI 或 API 匯入外部 OKF wiki，保留來源引用與連結。目前不支援重新匯入 Wenlan 自己匯出的 OKF。

### 瀏覽你的個人 Wiki

- **[可編輯、有來源的筆記](https://wenlan.app/docs/source-backed-pages)：** 把相關文件與記憶整理成 Markdown 筆記，附引用與筆記之間的連結。
- **卡片或列表：** 用適合自己的方式瀏覽筆記、實體與空間。
- **[知識圖譜](docs/technical-foundations.md#graph-data-and-entity-resolution)：** 探索人物、專案、主張與支撐記憶之間的關係。

### 搭配 AI 繼續工作

- **[換個 AI，也能接著用](https://wenlan.app/docs/architecture)：** 連接後，Claude Code、Codex 與其他 MCP 用戶端，可使用和桌面 app、CLI 相同的本地筆記與記憶。
- **[原詞與語意搜尋](docs/technical-foundations.md#retrieval-pipeline)：** 結合精確比對與本地語意搜尋，圖譜關係可補上相關脈絡。
- **[需要時，搜尋得更細](docs/technical-foundations.md#optional-channels-and-defaults)：** 可選擇搜尋筆記與更細緻的記憶，並重新排序結果。
- **[不同專案，分開管理](https://wenlan.app/docs/spaces)：** 用空間選擇 AI 這次要搜尋的工作、個人、客戶或程式庫知識。
- **網頁存取（實驗性）：** 電腦保持連線時，讓 Claude、ChatGPT、Muse 等網頁 AI 用戶端連到你的整個資料庫，或你選擇的一個空間。在設定裡打開「網頁存取」開關即可。查詢與結果會經過中繼服務，詳見[連接方式與隱私界限](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector)。
- **接入自己的工具：** 透過本地 HTTP API 傳入準備好的文字、網頁內容或記憶；它接收內容，不會代為抓取網址。

### 持續更新與維護

背景整理與筆記更新是可選功能，需要先[設定模型](#models-and-privacy)。

- **增量同步：** 檔案與資料夾會在背景追蹤變更；Obsidian 庫維持唯讀，需要時再同步。
- **[整理已存的知識](docs/technical-foundations.md#typed-memory-schema)：** 設定模型後，可協助補上類型、欄位、相關日期、標籤、搜尋提示與圖譜連結。
- **有引用才更新：** 自動更新會拒絕引用不足的草稿。AI 產生的筆記可以更新；你編輯過的筆記則先提出修訂。
- **[需要判斷才審核](https://wenlan.app/docs/review-and-trust)：** 審核受保護的衝突、筆記修訂、實體合併與新詞彙。
- **進度看得見：** 在「活動」查看進度與卡住的原因。本地服務運作時，關閉 app 視窗後仍可繼續執行已設定的同步、Steep（背景補全）與符合條件的筆記更新。

### 保有資料與控制權

- **[資料在本地，也能檢查](https://wenlan.app/learn/local-first-ai-memory)：** 保留 Markdown 筆記、引用、修訂與 Obsidian 匯出；記憶與圖譜存於本地 libSQL。
- **把 Wiki 帶走：** 從設定或 CLI，把各空間符合條件的筆記匯出成 OKF v0.2 wiki。這是 wiki 匯出，不是完整資料庫備份。
- **[模型自己選](docs/technical-foundations.md#model-roles)：** 基礎檢索留在本機。可選的 Steep（背景補全與筆記整理）能用裝置端 Qwen、本地端點或雲端模型；遠端服務會收到該任務所需的內容。
- **檢查問題，確認後修復：** [Doctor](https://wenlan.app/docs/diagnostics-and-issue-reports) 與 [lint](plugin/skills/lint/SKILL.md) 只回報問題，不改寫知識。支援的修復可在 app 預覽，確認後套用並驗證。

**先從一篇想留下的筆記開始。** [開始使用](#start-in-30-seconds)。想之後再試，也可以先給這個 repo 一顆 Star，方便回來找到它。


<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

## 日常流程

AI 工具連接好後，可以直接這樣說：

安裝 Wenlan 插件後，也能用快捷指令。

### 開始工作前

> 找出我存進 Wenlan 的［主題］相關資料，包括之前的決定與來源。

快捷指令：`/recall [主題]`

### 得到有用的結論時

> 把這個決定、背後的原因和來源記到 Wenlan。

快捷指令：`/capture [決定、理由與來源]`

<a id="值得整理成頁面時"></a>

### 值得整理成筆記時

> 把［主題］的已存資料整理成 Wenlan 筆記，已有的就更新，並保留引用。

將已存知識整理成筆記：`/distill`

在 Wenlan 裡閱讀、編輯筆記、查看來源。下次做相關工作時，請 AI 先讀這篇筆記，再接著做。

### 工作收尾時

> 把這次的進展、決定和待解決的問題記到 Wenlan，方便下次接著做。

快捷指令：`/handoff`

<details>
<summary>插件指令與維護</summary>

- **讀取專案摘要：** `/brief [topic]` 讀取目前空間的專案摘要；加上主題時，會補充同一空間的相關內容。
- **檢查與審核：** `/lint` 檢查知識庫狀態；`/curate` 審核待處理的記錄或修訂。

這些快捷指令由 Wenlan 插件提供。其他已連接的用戶端使用對應的 MCP 工具。可選的背景整理與筆記更新需要先[設定模型](#models-and-privacy)。

[完整指令參考](plugin/skills/README.md)。

</details>

<a id="離線佇列outbox"></a>

[CLI 離線寫入與重播](docs/setup-and-data.zh-Hant.md#offline-queue)。

<a id="models-and-privacy"></a>

### 模型與隱私

先用已連接的 AI 寫筆記，不必另裝本地語言模型。背景整理與自動更新是可選功能，需要另外設定模型。

- **搜尋在本機執行。** 首次啟動會下載搜尋模型，之後在你的電腦上運作，不需要 API key。
- **連接 AI，就可能傳出內容。** 雲端 AI 可將讀取的知識傳給其供應商；選用雲端模型整理資料，也會傳送該任務所需的內容。本機儲存不代表這些互動都留在本機。
- **使用統計預設關閉。** 同意後才傳送有限的操作計數、版本與平台，不包含知識內容或安裝識別碼。[查看說明](docs/PRIVACY.md#telemetry)。

模型下載、更新檢查、遠端圖片與選用遠端存取的連網行為，請見[隱私說明](docs/PRIVACY.md#when-wenlan-reaches-the-network)。

[模型選項與設定](docs/setup-and-data.zh-Hant.md#models)。

### 你的資料與移除

筆記與工作階段紀錄是 Markdown 檔案；記憶與圖譜保存在本地資料庫。移除 app 時，可以保留你的知識資料。

[檔案位置、備份與移除方式](docs/setup-and-data.zh-Hant.md#backup-and-removal)。


<a id="evaluation"></a>

## 評估

以下是 retrieval-only snapshot，不代表 end-to-end answer quality。方法、環境 receipts 與更新流程見 [docs/eval](docs/eval/README.md)。

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->


<a id="learn-more"></a>

## 進一步了解

更完整的文件、概念說明與比較：

### 文件

- [開始使用](https://wenlan.app/docs/get-started)：安裝並驗證第一個本地循環。
- [日常工作流程](https://wenlan.app/docs/daily-workflow)：brief、capture、recall、handoff、distill、lint 與 curate。
- [MCP 用戶端](https://wenlan.app/docs/mcp-clients)：連接 Claude Code、Codex、Cursor、Claude Desktop、Muse Code 與其他工具。

### 工作流程指南

- [建立第一個 LLM Wiki](https://wenlan.app/zh-TW/learn/distilled-wiki-pages-ai-memory)：用三份來源試做，核對引用，再檢查來源更新後哪些內容需要修改。
- [為顧問專案建立客戶知識庫](https://wenlan.app/zh-TW/learn/build-client-project-knowledge-base-for-consulting)
- [建立有來源支撐的投資研究知識庫](https://wenlan.app/zh-TW/learn/build-investment-research-knowledge-base)
- [在撰寫 PRD 前建立產品研究知識庫](https://wenlan.app/zh-TW/learn/build-product-research-knowledge-base-for-prd)
- [用 runbook 與事故復盤建立 SRE 事故知識庫](https://wenlan.app/zh-TW/learn/build-sre-incident-knowledge-base)
- [建立商業指標定義知識庫](https://wenlan.app/zh-TW/learn/build-business-metric-definition-knowledge-base)：把核准的 KPI 規格整理成有來源的資料字典，保留公式文字、粒度、排除條件、負責人、修訂與複核狀態。

### 概念

- [為什麼需要持續演進的 wiki，而不只是 AI 記憶](https://wenlan.app/learn/ai-work-memory)：深入理解問題與產品模型。
- [MCP 記憶伺服器](https://wenlan.app/learn/mcp-memory-server)：Wenlan 如何讓知識跨 AI 工具使用。
- [本機優先的 AI 記憶](https://wenlan.app/learn/local-first-ai-memory)：資料、隱私與控制權。
- [Markdown 與本地索引](https://wenlan.app/learn/local-first-ai-memory)：儲存、檢索與所有權。
- [AI agent 的交接循環](https://wenlan.app/learn/ai-agent-handoff-loop)：把工作完整帶到下一次會話。
- [用論文建立研究知識庫](https://wenlan.app/zh-TW/learn/source-backed-research-knowledge-base)：把已選好的論文整理成可檢查的文獻矩陣與來源支撐綜合。

### 比較

- [Wenlan 與 Basic Memory](https://wenlan.app/learn/wenlan-vs-basic-memory)
- [Wenlan 與 claude-mem](https://wenlan.app/learn/wenlan-vs-claude-mem)
- [Wenlan 與 Superlocal Memory](https://wenlan.app/learn/wenlan-vs-superlocal-memory)


## 貢獻

歡迎 bug fixes、eval cases、文件與功能。安裝 Wenlan 不需要從原始碼建置。本機開發時，請從本 repository 的根目錄執行以下命令：

```bash
# daemon crates（default-members——不會編譯桌面 app）
cargo build
cargo test

# 桌面 app（Cargo target 與根目錄的前端工具鏈）
pnpm install
pnpm dev:all
pnpm build:all
```

`pnpm dev:all` 是桌面 app 受支援的開發進入點。它讓開發用的連接埠、資料、行程歸屬、app 識別、MCP socket 與 Remote Access 狀態都與已安裝的正式執行環境隔離；未處於該隔離環境的 debug build 會拒絕啟動。完整開發流程見本 repository 的 [AGENTS.md](AGENTS.md) 與 [CONTRIBUTING.md](.github/CONTRIBUTING.md)，以及儲存庫內的 [app/AGENTS.md](app/AGENTS.md)。安全性問題請見 [SECURITY.md](.github/SECURITY.md)，隱私權政策請見 [PRIVACY.md](docs/PRIVACY.md)，也請閱讀 [Code of Conduct](.github/CODE_OF_CONDUCT.md)。


<a id="code-signing-policy"></a>

## 程式碼簽章政策

macOS 桌面版已通過 Developer ID 簽章與公證。Windows 安裝包尚未簽署。

發行版由 GitHub 託管的執行環境，依照此儲存庫的版本標籤與對應提交建置。維護者必須在 GitHub 啟用多因素驗證。

[各平台的簽章說明（英文）](docs/code-signing.md) · [隱私權政策（英文）](docs/PRIVACY.md)。


<a id="license"></a>

## 授權

Wenlan 採用兩種授權，依 repository 的不同部分劃分。

- **Apache-2.0**（[`LICENSE`](LICENSE)）涵蓋 local runtime、CLI、MCP server、shared types，以及 Claude Code 與 Codex 的 plugin files。可以自由基於這些開發。
- **AGPL-3.0-only**（[`app/LICENSE`](app/LICENSE)）涵蓋桌面 app：`app/` crate 及其附帶的 React 前端。如果你把修改過的 app 作為網路服務運行，AGPL 要求你向使用它的人提供這份修改後的原始碼。

這個劃分是刻意為之。Apache-2.0 的程式碼可以用在 AGPL-3.0 程式裡，所以桌面 app 建立在 runtime 之上，兩種授權都不會被違反。


<a id="acknowledgments"></a>

## 源流與同類專案

Wenlan（文瀾）的名字來自文瀾閣。這座皇家藏書樓收藏《四庫全書》，曾是中國最大的藏書之一。

Wenlan 的 llm-wiki v2 模型是自己的產品方向，並受到 LLM-wiki 與 agent-memory 兩條脈絡啟發：

- [Karpathy 的 LLM-wiki note](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) 建立了從 raw sources 到持續維護 wiki 的模式。
- [Rohitg00 的 LLM Wiki v2 proposal](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2) 加入 memory lifecycle、confidence、graph 與 retrieval mechanisms。[agentmemory](https://github.com/rohitg00/agentmemory) 是其具體的 agent-memory implementation。
- [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki) 是以文件為核心的 LLM-wiki 完整桌面實作。
- [basic-memory](https://github.com/basicmachines-co/basic-memory)、[obsidian-mind](https://github.com/breferrari/obsidian-mind)、[mcp-memory-service](https://pypi.org/project/mcp-memory-service/)、[Memoria](https://github.com/matrixorigin/Memoria) 和 [OpenMemory](https://github.com/CaviraOSS/OpenMemory) 探索相鄰的本地知識與 agent-memory 方向。
