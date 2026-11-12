# 設定與資料：安裝、模型、備份與離線佇列

本指南涵蓋安裝與更新、選用的模型支援背景工作、資料位置與安全保留，以及 CLI 離線 outbox。

語言：[English](setup-and-data.md) · [繁體中文](setup-and-data.zh-Hant.md) · [简体中文](setup-and-data.zh-Hans.md) · [Español](setup-and-data.es-ES.md)<br>
[回到繁體中文 README](../README.zh-Hant.md)

內容：[安裝](#installation) · [模型與隱私](#models) · [備份與移除](#backup-and-removal) · [離線佇列](#offline-queue)

<a id="installation"></a>

## 安裝

### 下載並開啟 Wenlan

[下載桌面版](https://github.com/wenlan-app/wenlan/releases/latest)，安裝後開啟：

- **macOS（Apple Silicon）：** 打開 `.dmg`，把 Wenlan 拖進「應用程式」。App 已簽署並通過公證。
- **Windows x64：** 執行 `-setup.exe`。安裝包尚未簽署。如果 SmartScreen 顯示警告，請先確認檔案來自官方 Releases 頁面，再點選「其他資訊」→「仍要執行」。
- **Linux x64：** 下載 `.AppImage`，設為可執行（`chmod +x Wenlan_*.AppImage`）後打開；或在 Debian、Ubuntu 上安裝 `.deb`（`sudo apt install ./Wenlan_*_amd64.deb`）。需要 Ubuntu 24.04、Debian 13、Fedora 40，或其他 glibc 2.39 以上且內含 WebKitGTK 4.1 的發行版。安裝檔沒有系統廠商簽章；之後會和其他平台一樣自動更新。ARM64 Linux 暫時沒有桌面版；可依照[設定指南](setup-with-ai.md#install-the-runtime)，不透過桌面 app，直接搭配 AI 工具使用。

首次啟動會下載本地搜尋模型，設定完成前請保持連網。[下載與隱私說明](PRIVACY.md#when-wenlan-reaches-the-network)。

### 其他安裝方式與更新

背景整理與自動更新是選用功能，需[另外設定模型](#models)；這與首次啟動下載的搜尋模型不同。

**從終端機安裝 macOS app**

安裝程式會下載 app、核對 SHA-256，並放進「應用程式」：

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/scripts/install-macos-app.sh)"
```

**不使用桌面 app**

在 macOS Apple Silicon 上執行：

```bash
npx -y wenlan setup
```

`npx` 需要 Node.js；若未安裝，可先執行 `curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash`，再執行 `wenlan setup --basic`。

這個命令會下載預先編譯的 CLI、背景服務（daemon）與 MCP 連接器，啟動並驗證本地服務；不需要安裝 Rust 或 Cargo。使用 glibc 的 Linux x64/ARM64 可以採用自動化的 [shell 設定流程](setup-with-ai.md#install-the-runtime)；Windows x64 請從 [Releases](https://github.com/wenlan-app/wenlan/releases/latest) 下載對應的 archive。macOS Intel 目前[沒有受支援的完整 runtime 安裝方式](../crates/wenlan-cli/README.md#macos-intel)。

**安裝內容與更新方式**

桌面 app 內建 daemon、CLI 與 MCP 連接器，開啟時會啟動 daemon，並提供偵測到的 AI 工具接入選項：Claude Code、Codex 使用 plugin，其他支援工具使用 MCP 設定。不使用桌面 app 時，執行的也是同一個 daemon；兩種方式都讓 AI 工具存取同一個本地知識庫。

更新 macOS app 時，把新 app 拖到舊 app 上覆蓋並開啟。Wenlan 0.17.0 及更早的版本需要先手動結束。

手動與各工具設定說明：[AI 輔助設定](setup-with-ai.md) · [Claude Code plugin](../plugin/README.md) · [Codex plugin](../plugin-codex/README.md) · [CLI 與 MCP](../crates/wenlan-cli/README.md)。

<a id="models"></a>

## 模型與隱私

先用已連接的 AI 寫頁面，不必另裝本地語言模型。背景整理與自動更新是可選功能，需要另外設定模型。

- **搜尋在本機執行。** 首次啟動會下載搜尋模型，之後在你的電腦上運作，不需要 API key。
- **連接 AI，就可能傳出內容。** 雲端 AI 可將讀取的知識傳給其供應商；選用雲端模型整理資料，也會傳送該任務所需的內容。本機儲存不代表這些互動都留在本機。
- **使用統計預設關閉。** 同意後才傳送有限的操作計數、版本與平台，不包含知識內容或安裝識別碼。[查看說明](PRIVACY.md#telemetry)。

模型下載、更新檢查、遠端圖片與選用遠端存取的連網行為，請見[隱私說明](PRIVACY.md#when-wenlan-reaches-the-network)。

### 模型選項與技術細節

- **本地基礎檢索：** [BGE 向量模型](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q) 透過 FastEmbed 在你的裝置上執行，用於混合搜尋，不需要 API key。
- **可選的裝置端整理：** 內容補充與頁面彙整可使用你選擇的 [`Qwen3.5 4B`](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) 或 [`Qwen3.5 9B`](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF)，透過 [llama.cpp](https://github.com/ggml-org/llama.cpp) 執行。未經選擇，不會下載或啟用語言模型。
- **其他模型來源：** Ollama 或 LM Studio 等 OpenAI 相容的本地端點，或已設定的雲端 provider，也可以提供模型支援的內容補充與頁面彙整。
- **在 Mac 上用 MLX：** 想用蘋果 MLX 格式的模型，可以在 LM Studio 或 Ollama 裡載入它，再到 Wenlan 設定裡選這個模型來源。內容補充、頁面彙整和逐句核對來源都會用它。
- **雲端說明：** 如果你選擇的模型端點位於遠端，Wenlan 會把該任務需要的 system prompt 與 user prompt 傳給它。本地檢索與裝置端整理仍留在你的裝置上。

完整 workflow 參考：[plugin/skills](../plugin/skills/README.md)。模型分工與限制見：[技術基礎（英文）](technical-foundations.md#model-roles)。

<a id="backup-and-removal"></a>

## 備份與移除

頁面與工作階段筆記是 Markdown 檔案；記憶與圖譜保存在本地資料庫。移除 app 時，可以保留你的知識資料。

### 預設位置

- 頁面與工作階段筆記：`~/.wenlan/`。
- 資料庫與執行環境資料：macOS 為 `~/Library/Application Support/wenlan/`，Linux 為 `~/.local/share/wenlan/`，Windows 為 `%LOCALAPPDATA%\wenlan\`。
- 以檔案保存的資料：資料庫資料夾裡的 `files-mirror/`。每個改動都會先寫進這裡的寫入紀錄（`files-mirror/log/`），再寫進資料庫；`files-mirror/files/` 裡可讀的檔案會在幾分鐘內跟上。資料庫只是這些檔案的索引：Wenlan 啟動時如果資料庫檔案不見了或是空的，會用這些檔案和寫入紀錄重建資料庫，連最後的改動也在，然後在背景重新檢查頁面和出處。檢查沿用 AI 之前的判斷，沒改過的內容不用再判斷一次。Wenlan 每天也會檢查一次這些檔案能重建出同樣的資料庫。請不要刪除這個資料夾。設定 `WENLAN_FILES_MIRROR=0` 可以關閉，那時就像以前的版本一樣只寫資料庫。

### 備份知識資料

複製資料夾前，先結束 app 並停止背景服務。自訂的頁面或資料位置也要備份。只匯出 wiki 不等於完整的資料庫備份。

若曾從 Origin 升級，請另外檢查 `~/.origin/` 與同層的 `origin` 平台資料夾是否留有舊資料。備份舊資料時一併保留；只有打算刪除時才移除。

### 移除程式

在設定關閉「登入時在背景執行Wenlan 」，結束 app，再刪除 `Wenlan.app` 或執行 Windows 解除安裝程式。知識資料夾會保留；只有確定不再需要資料，並已完成所需備份時，才另行刪除。

`wenlan background off` 會停止 daemon 並關閉自動啟動，但不會移除服務註冊。僅使用 CLI 的移除方式、AI 工具中殘留的設定、憑證與其他檔案，請見[完整移除說明](PRIVACY.md#data-deletion)。

<a id="offline-queue"></a>

## 離線佇列（outbox）

如果本機守護程序無法連線，`wenlan capture` 與 `wenlan brief update` 會把請求寫入本機持久化佇列（outbox）並正常結束。守護程序恢復後，它會透過一般 HTTP 路由排空這些寫入；用 `wenlan outbox status` 檢視佇列，或用 `wenlan outbox drain` 立即重播。被守護程序直接拒絕的寫入（4xx，例如未通過內容品質檢查）會連同回條移到 `outbox/failed/`，而不是無限重試；傳輸失敗或伺服器錯誤（5xx）則留在佇列中等待下一次排空，排空每 60 秒自動執行一次。
