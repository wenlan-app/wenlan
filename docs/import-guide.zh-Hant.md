# 把既有資料帶進 Wenlan

[English](import-guide.md) · [繁體中文](import-guide.zh-Hant.md) · [回到 README](../README.zh-Hant.md#開始使用)

開啟「設定 → 來源」，依手上的資料選擇匯入方式。

## 連接 Obsidian vault：唯讀

1. 在「已連接的來源」選擇「新增來源…」（尚未連接時為「新增第一個來源」）。
2. 瀏覽並選取 vault 資料夾。Wenlan 偵測到 vault 後，按「連接」開始建立索引。
3. 在 Obsidian 修改筆記後，按來源旁的「同步」，更新 Wenlan 的索引。

Wenlan 讀取 vault 裡的 Markdown 筆記，不修改原始檔案。**「同步」是將變更讀入 Wenlan，不會把修改寫回 Obsidian。** 你可以繼續在 Obsidian 編輯原本的筆記。

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
