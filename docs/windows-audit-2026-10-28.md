# Wenlan Windows 相容性總檢查 2026 10 28

初始檢查基準：`c6441196`；提交前已整合最新 main `f36db2fe`。Astra 負責核心、daemon、CLI 與桌面程式稽核；主代理補查建置腳本、CI 與本機工具鏈，並執行隔離驗證。

本次檢查 Wenlan 在 Windows 的路徑、程序、背景啟動、資料來源、檔案輸出、安裝依賴及防回歸機制。下面保留初始缺陷與重現條件；修正狀態和實際驗證範圍以本節為準。

這是程式庫與開發環境稽核，不是整台 Windows 的安全、磁碟或硬體健康認證。已執行隔離 daemon smoke；沒有更動使用者的背景服務、來源或正式資料。

## 修正與驗證狀態

Astra 共完成三輪複核。第三輪指出的「登記 root 與執行中舊程序不同」已以
移除破壞性 supervisor fallback 收斂：停止僅走 root-checked HTTP；Windows
task state/instance count、Linux ActiveState/MainPID、macOS loaded-job state/root
另行測量。執行中、排隊中或無法測量一律拒絕服務控制。macOS 更新要求舊 job
先卸載，避免 service-manager 內部的 remove/unload；啟動也不使用 kickstart -k。
更新的兩項舊 macOS HTTP 500 測試會檢查拒絕且不碰 supervisor。

初始基準最後修正後，本機定向驗證已通過：原生平台 86 項、CLI service 16 項、CLI
錯實例命令級測試 1 項、daemon startup 2 項、core 遷移 1 項、CI 路由與
禁止 force-stop 契約 95 項；原生 helper 嚴格 Clippy 及 diff/format 檢查通過。
隔離副本故意略過 Windows running-instance guard 後，對應的生產回歸測試確實
失敗；正式來源未作此破壞。複核止於約定的三輪，第三輪指出事項由後續修正及
上述驗證收斂，沒有宣稱 Astra 又做過第四輪或簽認修正後版本。

整合目前 main 後重新驗證：原生平台 71 項及嚴格 Clippy、CI 路由契約 101 項、前端 51 項通過。平台套件另外標記的 1 項是由父測試啟動的子程序 helper，不是略過的功能驗收。前端兩項曾在主機記憶體壓力下超時；釋放本次 TypeScript 檢查占用後，維持原超時設定復驗全部通過。初始基準的 UI/live smoke 收據保留為歷史證據；合併後版本以本段與 PR 的完整 CI 結果為準。

整合時另修正 main 的 Windows 編譯問題：GitHub wiki 模組只在 Unix 使用的
`Instant` 改為條件導入，沒有降低 `-D warnings` 或略過 Windows 檢查。

| 項目 | 已實作的修正與防回歸 |
| --- | --- |
| W01 | 整合後保留 main 在 config lock 內分配不碰撞來源 ID 的實作；舊 watch-path 遷移使用完整路徑雜湊且可重入。canonical 路徑去重；所有同步、HTTP 移除及 Tauri 舊入口拒絕重複 ID。交錯同步的資料庫測試檢查另一來源的記憶及 sync state 保留。 |
| W02 | 真實 home 初始化以選定資料根的檔案系統身分決定；repair 模式不建立根目錄，scratch 不寫預設 home，首次預設啟動保留初始化及舊目錄搬遷順序。smoke 先隔離 port/data/pages，再核對 daemon 回報。 |
| W03 | 保留 main 已加入的 remote_access_platform 原生 TCP owner table、程序身分及終止介面；新平台 lane 直接執行其生產模組與原生測試，沒有覆蓋回舊 remote_access 實作。 |
| W04–W05 | 排程 XML 分離 Command/Arguments，明確傳入 data-dir/bind/port，自訂根使用獨立 task 名稱。COM 解析真實產生的 XML；health/shutdown 增加資料根證明及 server 端重核。身份拒絕不允許 supervisor fallback。 |
| W06 | main 已補上原生 PATH 與 Windows 安裝目錄探索；保留其較新 plugin_install 實作和既有測試，不重複移植舊解法。 |
| W07 | 使用 console 原始按鍵安全讀取，Unicode backspace 可更正秘密；非 TTY 明確拒絕，不能降級為明文回顯。 |
| W08 | CLI 使用 open 的命令產生器及獨立目標參數／環境欄位，不再交給 cmd 解譯。生產 helper 測試覆蓋空白、中文及 shell 特殊字元。 |
| W09–W10 | Git Bash 先用 cygpath 正規化原生路徑；PowerShell 檔案操作使用 LiteralPath。實際 backend/SDK fixture 目錄包含 `[]`、空白及中文。 |
| W11 | 保留 main 的 KnowledgeWriter 長度、保留名稱與碰撞驗證；Obsidian 匯出增加 UTF-8 長度上限及 ID 雜湊。實際建立長 ASCII/CJK/emoji 檔名。 |
| W12–W13 | Obsidian registry 使用平台 config dir；共用路徑 basename 同時處理兩種分隔符。引用、匯出選單及手機預覽已驗證。 |

Windows 本機已通過資料來源資料庫測試、shutdown root guard、CLI 實例拒絕測試、原生平台 helper 測試，以及隔離 daemon 的儲存和語意檢索 smoke。前端 37 項測試及 TypeScript build 通過；Edge 桌面／手機預覽的中文 Windows 路徑顯示正常且無 page error。SDK、backend resolver 和既有 PowerShell harness 已執行。

daemon smoke 核對載入的是指定 `onnxruntime.dll` 與 `vulkan-1.dll`，並核對隔離資料／知識路徑。執行前後正式 `.wenlan`、`.origin`、pages、sessions 目錄 LastWriteTime 未變；這不是完整檔案內容快照，也不是 GPU GGUF 推論驗證。

新增的必要 `native-platform-contracts` CI lane 在 Windows/macOS/Linux 編譯並執行生產 helper；Windows 再執行 SDK 與 backend 路徑 fixture。此處沒有聲稱已在本機執行其他 OS，也沒有用 helper 測試代替完整 Tauri app、實際 task 註冊或簽署安裝包驗收。

### 既有資料與服務的安全限制

- 已經共用 ID 的來源會拒絕同步／移除，避免猜測資料歸屬。先停止確認過的實例，備份 config 與 memorydb，再為碰撞來源設定不同的新 ID；保留舊記憶與舊狀態後重新匯入。舊資料無法可靠反推出來源，可能形成重複記憶，需人工整理，不能自動刪除。
- 舊 daemon 不回報資料根證明，或服務登記沒有可靠 root 時，拒絕自動控制，並指向 [一次性服務升級步驟](cross-platform.md#legacy-service-upgrade)。這是有意保留的相容性限制；本輪沒有完整自動跨版本升級。
- 真實 Task Scheduler 註冊／查回／執行，以及 ZIP/NSIS 首次安裝須在可丟棄的 runner 驗證；本機只做不註冊 task 的 COM XML 解析。
- 乾淨無 vendor ICD VM、真實 GPU Qwen/GGUF 三條路徑仍按 [Windows Vulkan runbook](windows-vulkan.md) 驗收；本輪未執行。

以下為初始稽核記錄，其中「修正方向」描述原始缺陷的驗收依據，並非未完成清單。

## 問題總覽

| 編號 | 等級 | 問題 | 證據 |
| --- | --- | --- | --- |
| W01 | P1 | 同名但不同路徑的來源共用 ID，同步可刪掉另一來源的匯入資料 | 程式鏈確認 |
| W02 | P1 | 自訂資料與知識目錄仍不能隔離啟動時對真實 home 的寫入 | 程式鏈確認 |
| W03 | P1 | Windows 遠端連線依賴 `lsof`、`ps`、`kill` | 程式鏈確認，缺少工具已實測 |
| W04 | P2 | 排程工作執行字串未替含空白的 executable 路徑加內層引號 | 靜態確認，task action 未實測 |
| W05 | P2 | Windows 排程工作沒有保存自訂資料目錄與綁定位址 | 程式鏈確認 |
| W06 | P2 | 桌面插件安裝只透過 Unix shell 與 Unix 路徑找 CLI | 程式鏈確認 |
| W07 | P2 | Windows API key 輸入無法關閉回顯 | 程式鏈確認，缺少工具已實測 |
| W08 | P2 | CLI 開啟頁面把合法路徑交給 `cmd /C` 再解譯 | 同型 Rust 子程序 probe 重現 |
| W09 | P2 | 原生 Windows backend 絕對路徑被當作相對路徑 | Git Bash 實機重現 |
| W10 | P2 | PowerShell 把合法目錄中的 `[]` 當 wildcard | SDK 驗證實機重現 |
| W11 | P2 | 頁面標題直接成為無長度上限的檔名 | 程式鏈確認，檔案系統限制已實測 |
| W12 | P3 | Obsidian 自動探索固定讀 macOS registry 路徑 | 程式鏈確認 |
| W13 | P3 | 來源與引用標籤只用 `/` 拆檔名，未處理 Windows 反斜線 | 程式鏈確認 |

P1 應先處理資料影響與功能阻斷；P2 包含有明確觸發條件的錯誤與待實測的執行介面風險；P3 影響便利功能。W01、W02、W11 也涉及其他平台，仍列在這次 Windows 總檢查內。

## 資料與執行隔離

### W01 同名來源會共用同步狀態

位置：[source_routes.rs](../crates/wenlan-server/src/source_routes.rs)，`handle_add_source` 132–144、`sync_directory_source` 367–384 與 554–559、`handle_remove_source` 174。

來源 ID 只有 `type + slug(basename)`，重複檢查卻只比較完整 path。因此 `C:\A\Notes` 與 `C:\B\Notes` 都可註冊成 `directory-notes`。同步先依 ID 讀取全部 tracked paths，再和當前單一目錄的掃描結果比較；另一目錄的檔案可被判成消失，進入 `delete_by_source_id`、`delete_sync_state` 與 `dequeue_document`，即使原檔仍存在。刪除來源設定也會一次移除同 ID 的兩筆。

修正方向：為新來源產生不碰撞的持久 ID；另以平台正確的 canonical identity 去重。既有碰撞需要同步狀態遷移或明確重建，不能只改 ID 字串。

驗收：註冊兩個同名目錄，交錯同步、修改、移除其中一個，另一個的設定、記憶、佇列與同步狀態全部保留；Windows 增加大小寫與路徑別名案例。

### W02 Scratch daemon 啟動仍會寫真實 home

位置：[startup.rs](../crates/wenlan-server/src/main/startup.rs)，167–309。

啟動 consolidation 直接使用 `dirs::home_dir()`，建立 `~/.wenlan/{pages,sessions,sessions/_status}`，可從 `~/Origin/knowledge` 複製 Markdown，並在 `~/.wenlan` 執行 Git 初始化及加入檔案。這段沒有按 `WENLAN_DATA_DIR` 或 `config.knowledge_path` 選擇目的地。因此即使測試隔離 port、資料目錄與知識目錄，仍會碰到真實 home。Windows 的 home 解析也不能假設可由 POSIX `HOME` 覆寫。

修正方向：把啟動搬遷的所有根目錄注入同一個 runtime paths 物件；自訂資料根時，明確跳過對預設 home 的 consolidation，或要求可驗證的獨立 home 根目錄。

驗收：以 scratch runtime 啟動後，對預設 home 的 sentinel、目錄樹與 Git 狀態逐項證明無變更；包含「存在舊版 Origin 頁面」與「預設 `.wenlan` 尚不存在」兩種條件。先修這項，再執行新的本機 live daemon smokes。

`scripts/smoke-windows.ps1` 目前只有臨時 data dir、固定 port 17878，沒有先寫 scratch `knowledge_path`、驗證 daemon 回報根目錄或設 `WENLAN_NO_AUTOSTART=1`。補齊 smoke 隔離必須與上述 startup 修正一起驗收。

## 程序與背景啟動

### W03 遠端連線以 Unix 程序工具驗證 Windows 子程序

位置：[remote_access.rs](../app/src/remote_access.rs)，`listener_pid_for_port` 683、MCP readiness 1102、`cloudflared_process_identity` 657、`record_cloudflared_owner` 504 與呼叫點 1612、orphan cleanup 308。

MCP 的 `/health` 成功後仍必須通過 `lsof` 的 listener PID 比對。原生 Windows 沒有這個 executable，probe 回傳 `None`，啟動逾時後終止子程序。即使只修 listener，cloudflared 的 ownership receipt 又必須經 `ps` 取得程序 identity，仍會失敗。清理程序還依賴 Unix `kill`。

同一模組的 command identity 以 `split_whitespace()` 拆 executable；安裝路徑含空白也不能被可靠還原。

修正方向：集中實作平台程序介面，使用 Windows listener ownership API 與已有的 `sysinfo` 取得 PID、啟動時間、executable 和 argv；保留 ownership 與 PID reuse 防護。不能以略過 ownership 檢查讓 readiness 通過。

驗收：Windows 啟動隔離的假 MCP listener 與假 cloudflared，走真實 readiness、receipt、停止與 orphan cleanup。包含其他程序佔用 port、PID identity 不符、含空白與繁中安裝路徑；測試不得依賴 Git Bash 提供 Unix 工具。

### W04 排程工作沒有引用 executable 內部的空白

位置：[service.rs](../crates/wenlan-cli/src/commands/service.rs)，399–414。

`schtasks /create /tr` 收到的是裸 `program_str`。把它作為 `Command::arg` 的單一參數，只處理呼叫 schtasks 時的 argv 邊界；`/tr` 的內容之後仍是另一層執行字串。安裝目錄或使用者名稱含空白時，沒有保留 executable 所需的內層引號。

本輪未註冊 task 或查回實際 action；已確認程式傳入裸字串，含空白時 schtasks 實際如何拆解，仍需本機往返驗證。

修正方向：優先用 Task Scheduler XML 的獨立 `Command` 與 `Arguments` 欄位，避免多層 command line 引用。

驗收：在可丟棄的 Windows runner，用包含空白及繁中的 executable 路徑註冊、查回 action 並啟動。只在確認工作屬於本輪後清理；禁止在開發者機器註冊或刪除共用 `WenlanServer`。

### W05 排程工作沒有保存呼叫端的 runtime 設定

位置：[service.rs](../crates/wenlan-cli/src/commands/service.rs)，388–425；`explicit_data_root` 281、健康檢查綁定位址 867。

Windows 註冊的 action 只有 executable，沒有持久化 `WENLAN_DATA_DIR`、`WENLAN_BIND_ADDR` 等程序層設定。排程服務啟動的 daemon 不會取得僅在此次 CLI 程序中設定、未保存到使用者或系統環境的值，CLI 卻仍按呼叫端設定輪詢健康與管理 marker。例如只為該次 CLI 設 scratch `WENLAN_DATA_DIR` 與 `WENLAN_BIND_ADDR=127.0.0.1:17917`，背景程序便缺少相同選擇。自訂資料根或 port 因而可能啟動到預設 runtime，或輪詢到另一個實例。macOS 同檔已有顯式資料根的持久化處理，Windows 路徑沒有對應實作。本輪由 action 與程序啟動邊界確認設定缺口，沒有執行 Task Scheduler 往返。

修正方向：定義背景工作的持久 runtime 設定與 instance identity，讓 action、健康檢查、marker、停止程序使用同一份設定；自訂根目錄不得覆寫或終止預設實例。

驗收：從只存在於程序環境的 scratch data dir 與非預設 port 註冊背景工作；清除父程序環境後啟動，檢查健康回應中的資料根、知識根與 PID。預設實例的 port、marker 與資料必須保持不變。

### W06 桌面插件安裝找不到原生 Windows CLI

位置：[plugin_install.rs](../app/src/plugin_install.rs)，60–66、91–115；桌面呼叫點：[search.rs](../app/src/search.rs)，448 起。

解析器先執行 `zsh -lic`，失敗後只找沒有 Windows suffix 的 `.local/bin/claude`、`.claude/local/claude`、Homebrew 等路徑。原生 `claude.exe` 與 npm 的 `codex.cmd` 不會被找到，導致安裝插件時錯報 `CLI not found`。

修正方向：Windows 使用 PATH 與 PATHEXT 解析，納入原生安裝目錄；對 `.exe` 與 `.cmd` 分別使用正確的啟動方式。

驗收：以注入的 Windows PATH 測試 `.exe`、`.cmd`、路徑空白、找不到 CLI 與 child 非零 exit；必須走生產 resolver 與實際啟動邊界。

### W07 API key 輸入在 Windows 保持回顯

位置：[setup.rs](../crates/wenlan-cli/src/commands/setup.rs)，`prompt_secret` 1018–1027；呼叫點 192、379。

關閉及恢復回顯只呼叫 `stty`，且忽略失敗。原生 Windows 沒有這個 executable，console 仍可把輸入的 Anthropic key 顯示在畫面與捲動紀錄中。

修正方向：採用跨平台的 secret input 或 Windows console mode API，並在錯誤及中斷時恢復 terminal state。

驗收：使用假密鑰與可丟棄的 Windows console，驗證輸入不出現在可讀畫面，成功、錯誤和中斷都恢復 console mode；不要以真正金鑰做測試。

### W08 開啟頁面會再次解譯路徑中的 cmd 語法

位置：[pages.rs](../crates/wenlan-cli/src/commands/pages.rs)，145–151。

CLI 執行 `cmd /C start "" <path>`。合法的無空白路徑若含 `&`，可以被當成命令分隔符。隔離 Rust probe 保留同樣 `Command` argv 形式、把 `start` 換成無 GUI 的 `echo`，傳入 `C:\Notes&AUDIT_ECHO.exe&report.md`，輸出在 `C:\Notes` 截斷，並分別嘗試執行不存在的 `AUDIT_ECHO.exe` 與 `report.md`。這證明路徑經過 shell 再解譯；本次沒有執行額外的有效命令。

修正方向：與桌面 `open_file` 一致，使用 OS 開檔介面或跨平台 open library，避免自行拼接 `cmd` action。

驗收：開檔 adapter 接收含空白、繁中、`&`、括號、`%` 的完整原始 path；以 mock launcher 證明只派送一次開檔操作，沒有額外命令或環境變數展開。

## 路徑與檔案

### W09 Backend resolver 誤判磁碟機絕對路徑

位置：[resolve-backend-dir.sh](../scripts/resolve-backend-dir.sh)，18–26。

腳本只把 `/` 開頭視為絕對路徑。實測將 `WENLAN_BACKEND_DIR` 設為本 checkout 的正常 `C:\Users\…\wenlan`，Git Bash 把它接成 `/c/Users/…/wenlan/C:\Users\…\wenlan`，然後回報不是 backend checkout，exit 1。

修正方向：在 shell 邊界把原生 Windows 路徑轉成 shell spelling，才判斷相對路徑；保留非 Windows 上原本的行為。沿用單一轉換介面，檢查所有 path override 的消費者。

驗收：同一真實 fixture 分別使用 `C:\...`、`C:/...`、MSYS `/c/...`、含空白與繁中路徑及相對路徑，解析到同一 backend。UNC 與 extended-length 路徑另列原生 Windows 案例，不能從磁碟機案例推定通過。

### W10 SDK 驗證把目錄名稱當 wildcard

位置：[setup-vulkan-sdk-windows.ps1](../scripts/setup-vulkan-sdk-windows.ps1)，56、62、67、74。

以合法 `[Wenlan] 中文 SDK` 目錄建立三個必要檔案，逐個用 `Test-Path -LiteralPath` 都得到 `True`；生產 `-ValidateOnly` 卻因 `Test-Path $_` 把中括號當 pattern，回報 SDK 不完整。

修正方向：對具體使用者路徑使用 `-LiteralPath`，只在程式明確要展開 glob 的地方使用 `-Path`；一併檢查 Resolve、Copy、Hash 與清理邊界。

驗收：SDK 與 DLL staging fixture 全程置於含空白、繁中、`[]` 的根目錄；驗證檔案、hash 與 cleanup 的目標都正確。這次只有 SDK 誤判完成重現，不把其他腳本的同類寫法一概宣稱為已重現。

### W11 標題直接產生超過檔案系統限制的檔名

位置：[page_create.rs](../crates/wenlan-core/src/post_write/page_create.rs)，273、425；[knowledge.rs](../crates/wenlan-core/src/export/knowledge.rs)，250、398–432；[obsidian.rs](../crates/wenlan-core/src/export/obsidian.rs)，78。

頁面建立只檢查 title 非空；writer 使用完整 `slugify(title) + .md`，沒有 component 長度預算。在本機 Windows，256 個 `a` 再接 `.md` 的 std 檔案寫入 probe 回傳 OS error 123。生產 writer 在 metadata 或寫入時也會碰到同一限制。完整 path 的限制與單一檔名 component 的限制必須分開處理。

修正方向：保留完整顯示標題，檔名使用有界 slug 加穩定 ID/hash，預留 extension 與碰撞 suffix 空間，並按 UTF-8／UTF-16 邊界安全截斷。

驗收：短標題、超長英文、繁中、emoji、截斷後碰撞及更新既有頁面；在 Windows 實際寫入並重新讀取。長路徑支援另驗證，不能用它取代 component 上限測試。

### W12 Obsidian 探索固定使用 macOS 路徑

位置：[obsidian.rs](../app/src/sources/obsidian.rs)，102–106；呼叫點：[search.rs](../app/src/search.rs)，1297。

`obsidian_registry_path()` 在所有平台都回傳 `~/Library/Application Support/obsidian/obsidian.json`。Windows 因讀不到此檔而回傳空清單，無法自動提出既有 vault；手動選取資料夾仍可使用。

修正方向：按平台解析 Obsidian 的設定根目錄，將路徑 resolver 與 registry parser 分開測試。

驗收：Windows、macOS、Linux 分別給定設定根目錄，確認讀取對應 fixture；缺檔及損壞 JSON 繼續安全地回傳空清單。

### W13 來源與引用標籤沒有處理反斜線

位置：[dateFormat.ts](../src/lib/dateFormat.ts)，37–39；[pageCitations.ts](../src/lib/pageCitations.ts)，119–120。

`folderName` 與 `citationDisplayLabel` 只按 `/` 分隔。原生 `C:\Notes\report.md` 會整段顯示，無法產生預期的短檔名或目錄名稱。來源清單和頁面引用都會受影響。

修正方向：共用 display-basename helper，辨識兩種分隔符；URL 繼續用 URL parser，不能用同一個字串替換規則混處理。

驗收：drive path、UNC path、兩種分隔符與 trailing separator 都回傳正確短名；保留原始 locator 供真正開檔使用。

## 防止重犯的驗收設計

目前必要 CI 沒有涵蓋完整 Windows 行為：[ci.yml](../.github/workflows/ci.yml) 1252 起明確略過 Windows workspace lib tests；`app-check` 2425 起只在 macOS 執行；`app-windows-bundle` 2499 起限手動 dispatch，且未列入必要 `conclusion`。原生程序工具問題即使能在 Windows 編譯，也不會自動被編譯檢查抓出。

不要把所有檢查塞回每次 push 的整個 workspace。應建立依 owner 路由的 Windows 行為測試集，並讓 aggregate `conclusion` 驗證預期的 job 確實執行。

| 防線 | 必須實際驅動的生產邊界 | 必須抓到的退化 |
| --- | --- | --- |
| 路徑表格案例 | backend resolver、SDK staging、CLI opener、filename writer | 原生 path 被拼錯、wildcard、shell 再解譯、component 超長 |
| 程序介面 | MCP readiness、listener PID、argv identity、ownership cleanup | Windows 回退到 `lsof`／`ps`、空白路徑拆錯、PID 不符仍 kill |
| 來源獨立性 | add → 交錯 sync → remove | 同 basename 共用 ID、另一 root 被視為 disappeared |
| 啟動隔離 | runtime paths → consolidation → migration | scratch 啟動寫入預設 home、資料或知識根 |
| 背景工作 | action 定義 → 父環境清除 → start → health → stop | 引號遺失、data/port 遺失、管理到其他實例 |
| Windows CLI 安裝 | PATH／PATHEXT resolver → child argv → exit | `.exe`／`.cmd` 漏找、失敗被當成功 |
| 密鑰輸入 | native console mode 與所有離開分支 | key 出現在 console、錯誤後回顯未恢復 |

測試必須呼叫生產實作。只檢查原始碼字串、注入一個總是成功的 resolver、或把缺失標成 skip，都不能代替 Windows 行為驗收。每個重要修正至少做一次反向控制：在隔離副本撤回該修正，原本保護它的測試必須失敗。

建議修正順序：

1. 先修 W01、W02，確保資料來源獨立與測試隔離成立。
2. 補原生程序介面，修 W03、W06、W07、W08，連同 Windows 必要測試一起接入 CI。
3. 修 W04、W05；排程與安裝往返只在可丟棄的 runner／VM 驗收。
4. 修 W09、W10、W11、W12、W13，讓上述路徑案例共用同一組測試資料。
5. 最後從真實 release ZIP 與 NSIS 做首次安裝、升級、背景啟動與卸載；另做無 vendor ICD 的真 GGUF CPU inference 驗證。

這些是待實作的防線，不代表本輪已接入 CI 或已修復產品。

## 本次驗證紀錄

| 檢查 | 結果 | 可支持的範圍 |
| --- | --- | --- |
| `setup-msvc-ninja-windows.test.ps1` | PASS | stub MSVC／Ninja 環境與設定保留契約 |
| `setup-vulkan-sdk-windows.test.ps1` | PASS | 既有 SDK fixture 與驗證契約 |
| `stage-vulkan-loader-windows.test.ps1` | PASS | fixture archive、hash、staging 契約 |
| `smoke-windows-llm.test.ps1` | PASS | 假 model probe 的成功、錯誤與 CPU／GPU marker 判讀 |
| `ci_test_plan.test.py` | 93 tests PASS | CI planner 既有路由與命令契約 |
| `release_targets.test.py` | 12 tests PASS | 四個 shipped target 的既有發布 inventory |
| PowerShell parser | 16 個 `.ps1` PASS | 語法可解析，不代表行為正確 |
| Git Bash 原生 backend path probe | 預期錯誤重現 | W09 |
| 中括號 SDK fixture probe | 預期錯誤重現 | W10 |
| Astra std-only Windows probes | 完成 | 缺少 Unix executable、cmd 分隔符、檔名 component 上限；未啟動產品 daemon |

所有本次測試與 probes 設 `WENLAN_NO_AUTOSTART=1`。四組 PowerShell 契約測試採 fixture，沒有下載模型或驗證真實 GPU。沒有執行 Windows ZIP／NSIS 首跑腳本、註冊服務、啟動真實 daemon、修改使用者資料或套用系統設定。

Rust／桌面完整編譯、真實 SQLite 檔案鎖競態、watcher 事件時序、UNC／extended-length 路徑、NSIS 升級往返、真 GGUF、driverless VM 仍為 unchecked。`docs/windows-vulkan.md` 已記載無 ICD 路徑尚無真 GGUF 驗證，不能從現有有驅動環境的測試推定通過。

另外檢查了 MCP 設定的 `.exe` 路徑解析、daemon sidecar 的 job object 與 PID／start-time 防護、備份在 rename 前關閉驗證連線，以及 projection 的同目錄原子替換；本次沒有找到與上述問題同等明確的新缺陷。這些是程式碼審查結果，不代表 crash、外部鎖檔或完整發布流程已實測通過。

繁中加空白檔名的 Rust 寫入 probe 成功；`nul.md`、`con.md`、`aux.md` 在這次使用的 Rust API 也可建立成 regular file，因此沒有把 DOS 保留名列為已重現故障。Windows presence minting 已明確回報 platform unsupported，屬已宣告的功能限制。

## 這台開發環境的額外發現

- `core.hooksPath=.githooks` 已查證；只代表 hooks 有設定，不代表其中呼叫的工具確實執行。
- PowerShell 的 `python`／`python3` 先解析到 pyenv `.bat` shim。未設定 runtime 時，曾輸出「No global/local python version has been set yet」並回傳 0；後續 `-c print('WENLAN_PYTHON_RUNTIME_OK')` 也沒有印出 sentinel，仍回傳 0。不能把這種 exit 0 記為測試通過。本輪改用已確認的 bundled Python executable 跑完上述 105 個測試。建議 developer preflight 以 sentinel 與版本確認 interpreter，並讓 hooks 共用通過驗證的 interpreter resolver。
- PATH 中的 `bash` 指向 `C:\Windows\System32\bash.exe`，即 WSL 入口。現有 `run-bash.mjs` 已顯式尋找 Git Bash；手動命令及其他測試也要使用同一入口，不能因命令叫 `bash` 就認定是 Git Bash。
- checkout 起始沒有 `node_modules` 或 `target`。`pnpm --version` 的 Corepack 嘗試取得 pinned 10.28.2，但目前沙箱網路 DNS 拒絕下載；因此沒有把 Vitest 或 Tauri build 記為通過。
- Git Bash 初次被沙箱阻擋建立 MSYS named object；Python 初次也遇到沙箱暫存目錄 ACL 問題。相同唯讀路徑 probe 與 planner 測試在核准的沙箱外執行後取得上表結果；這兩種沙箱限制不算 Wenlan 產品缺陷。
