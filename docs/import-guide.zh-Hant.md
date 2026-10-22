# 把既有資料帶進 Wenlan

[English](import-guide.md) · [繁體中文](import-guide.zh-Hant.md) · [回到 README](../README.zh-Hant.md#開始使用)

開啟「設定 → 來源」，依手上的資料選擇匯入方式。

## 連接 Obsidian vault：唯讀

1. 在「已連接的來源」選擇「新增來源…」（尚未連接時為「新增第一個來源」），再選「連接 Obsidian 庫」。
2. 瀏覽並選取 vault 資料夾。
3. 選擇「持續同步」或「只匯入一次」：
   - **持續同步**（預設）：繼續在 Obsidian 編輯就好。Wenlan 執行時約每 30 秒檢查一次 vault 的變更，包括修改、新增、改名和刪除的筆記。
   - **只匯入一次**：Wenlan 現在讀取一次。之後的變更，在來源資訊裡按「重新整理」才會讀入。

   之後可以在來源資訊裡用「持續同步」開關切換。想馬上更新，也可以在那裡按「重新整理」。

Wenlan 讀取 vault 裡的 Markdown 筆記，不修改原始檔案。**變更只會讀入 Wenlan，不會寫回 Obsidian。**

## 連接 Logseq 圖譜：唯讀

1. 選擇「新增來源…」，再選「連接 Logseq 圖譜」，選取圖譜資料夾（裡面有 `pages/`、`journals/` 和 `logseq/` 的那個）。
2. 跟 Obsidian 一樣，選擇「持續同步」或「只匯入一次」。持續同步時，你繼續在 Logseq 寫，Wenlan 約每 30 秒跟上修改、新增、改名和刪除的頁面。

Wenlan 讀取 `pages/` 和 `journals/` 裡的 Markdown 頁面。大綱會整理成一般段落和清單，日記用日期當標題，`id::` 這類區塊屬性會略過。Logseq 放在 `logseq/bak/` 的備份不會讀取，也不會寫回 Logseq。新版 Logseq 資料庫版把頁面存在資料庫而不是檔案裡，請先匯出成 Markdown 再連接。

## 匯入 ChatGPT／Claude 對話 ZIP

1. 在 AI 帳戶中申請匯出：
   - **ChatGPT：** 設定 → 資料控制 → 匯出資料。[官方說明](https://help.openai.com/en/articles/7260999-exporting-your-chatgpt-history-and-data)。
   - **Claude：** 設定 → 隱私 → 匯出資料。[官方說明](https://support.claude.com/en/articles/9450526-export-your-claude-data)。
2. 下載匯出檔，將 ZIP 拖入「匯入聊天記錄」，或在該處選取檔案，不必解壓縮。
3. 查看匯入結果。先前匯入過的對話會略過，後續處理可能仍在背景進行。

這裡匯入的是成對的使用者與助理文字，不含附件或其他多模態內容。若 Claude 提供的是 manifest JSON，請依清單下載對話 ZIP 並分別匯入；manifest 本身無法匯入。這是匯入當次匯出的紀錄，不會持續連接你的 AI 帳戶。

## 貼上文字匯入記憶

1. 在「匯入記憶」選擇「貼上或匯入」。
2. 選擇 ChatGPT、Claude 或其他來源，貼上想保留的記憶，**每行一則**。需要時可使用表單提供的提示詞，請 AI 匯出它記住的內容。
3. 檢查文字後按「匯入」，再開啟「查看記憶」，確認保存的內容。

這種方式適合已有的簡短偏好、決定、事實或經驗；完整對話紀錄請用上方的 ZIP 匯入。

## 透過 AI 工具存下郵件、頁面或討論串

如果你的 AI 工具讀得到 Gmail、Notion、Google Drive、Slack、GitHub 或網頁，可以請它把讀到的內容存進 Wenlan，例如「把這封信存進 Wenlan」。接上 Wenlan 外掛後，工具會連同原文連結一起存下，引用它的筆記就能點回原本的郵件或頁面。也可以用命令列存一份：`wenlan sources save --url <連結> --title <標題> --label gmail < email.txt`。

Wenlan 保留存下的那一份，不會再去抓原文。原文更新後，請工具用同一個連結再存一次，Wenlan 會換成新的內容，並把引用它的筆記標記為需要更新。這要用本機工具，例如 Claude Code 或 Codex；claude.ai 和 ChatGPT 的網頁存取是唯讀的，無法存來源。
