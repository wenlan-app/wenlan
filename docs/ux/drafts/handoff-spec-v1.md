# 交接體驗 spec v1（草稿，待 Astra 6.2 審查）

日期：2026-11-20。依據：wenlan-app/wenlan main `c788a460`（2026-11-20 18:38Z）。
需求原文（Qi-Xuan，2026-11-20）：
- 「交接為啥總是失敗，這不能根本解決嗎」
- 「這樣就失去交接的意義？如果一天內有好多人在做事，可能其他人就看不到？」
- 「從 UX/DX 根本去想這個體驗。對先不用等個人雲端，以及這個 brief 最後會回到 space 裡面的 UI 頁面對吧，也都會在 cli mcp 等等？這點沒有一致對齊。」
- 「還有用戶記得要 handoff 自然是最好，要是他們累了或忘了怎麼辦，重複交接呢等等」
- 「建議要把缺口等完整的 spec 寫好（可以讓 Astra 6.2 審查」
- 「一個好時間把 UX/DX 做到簡潔好懂省心」
- 已同意（19:46Z）：交接主要給「下一個接手的 AI」看（雲端對話或 Mac 上的 Codex 都算），人只看一行摘要。

## 0. 一句話

**交接是 Wenlan 的事，不是人的事，也不是腳本的事。** 每個 Space 只有一份「現在到哪」（Brief）；任何工具收工時更新它、開工時自動拿到它；人什麼都不用記，打開 Space 就看得到。

```
任何 AI 工具收工 ─(自動，或叫 handoff)─> Wenlan：這個 Space 的「現在到哪」(一份)
                                              │
下一個 AI 開工 <──────── 自動帶出 ─────────────┤
人打開 Space 頁 <──────── 一眼看到 ─────────────┘
忘了交接 ──> 晚上從對話本身補寫
連不上 ──> 先排隊，連上就補，不會丟、不會重複
```

## 1. 誰讀、何時、在哪

| 讀的人 | 何時 | 在哪 | 需要什麼 |
|---|---|---|---|
| 下一個接手的 AI（主要） | 開新對話的第一句之前 | 它所在的工具（Claude Code、Codex、其他 MCP 工具、雲端對話） | 這個 Space：做到哪一兩句、進行中的事（每件一行）、卡在誰、下一步 |
| 人（Qi-Xuan、以後的隊友） | 想知道進度時 | app 的 Space 頁最上面（UX-SPACE-1，已上線） | 一兩句人話；誰、多久前交接 |
| 同時在做事的其他 AI | 做到一半 | 同上 | 別人剛改了哪一條（不互相蓋掉） |

不讀交接的地方：首頁「正在做」只放從對話讀出的事（UX-HOME-6，已定），本 spec 不改這條邊界。

## 2. 現在怎麼運作（盤點）

| 入口 | 寫交接 | 讀交接 | 依據 |
|---|---|---|---|
| app | 人不寫 | Space 頁最上面「現在到哪」；對話卡片「交接摘要」 | `src/components/memory/space-detail/SpaceBrief.tsx`，`sessionCopy.ts handoffTitle`，UX-SPACE-1/2 |
| CLI | `wenlan brief update --file`（daemon 沒開時排進 outbox，開了自動補送） | `wenlan brief` | `crates/wenlan-cli/src/main.rs:145`，`crates/wenlan-server/src/outbox.rs`，`main/runtime.rs:230` |
| MCP | `update_brief` | `brief`（說明寫「resume 時讀」，由 AI 自己決定） | `crates/wenlan-mcp/src/tools.rs:7043, 8313` |
| HTTP | `PATCH /api/brief`（需 `caller_id` + `operation_id`，同一 operation 只算一次；每條有 id + version，舊版本會 conflict） | `POST /api/brief`、`/api/brief/history` | `brief_routes.rs:20`，`wenlan-core/src/db/brief.rs:97, 331-351` |
| Claude Code | 技能 `/wenlan:handoff`：8 步 bash + MCP 腳本（Brief、captures、`~/.wenlan/sessions/*.md` 敘事紀錄、更新用過的頁、回報） | 開場 hook 只印 session id，不帶 Brief | `plugin/skills/handoff/SKILL.md`（265 行），`plugin/hooks/session-context.sh` |
| Claude Code 結束 | 無 | — | `plugin/hooks/hooks.json`：Stop hook 只跑「違反以前決定」檢查（`stop-check.sh`）；沒有 PreCompact / SessionEnd |
| Codex | 技能 `$wenlan handoff`（同一份腳本的第二份拷貝） | 要問「上次做到哪」 | `plugin-codex/skills/handoff/SKILL.md`（269 行）；`plugin-codex` 沒有 hooks |
| 其他工具 | 第三份拷貝 | 同上 | `plugin-agents/skills/wenlan-handoff/SKILL.md`（258 行） |
| 雲端對話（claude.ai 專案） | 寫 md 到 `/mnt/project-files/<topic>/`；進 Wenlan 要走 claude.ai 連線 → relay.wenlan.app → Mac，常斷；雲端沒裝技能 | 讀專案資料夾、專案記憶 | 137 份交接檔，至少 10 份記錄「沒存進 Wenlan」，原因：Mac 睡著、連線登入過期、relay KV 額度、連線唯讀、MCP 不穩、沒技能 |
| 對話本身 | Wenlan 已經自己讀 Claude Code / Codex 在本機的對話紀錄，拿來分「正在做的事」 | — | `wenlan-core/src/conversation_intake.rs`（UX-THING-1） |

## 3. 缺口

- **G1 交接不是 Wenlan 的一個動作。** 邏輯在技能腳本裡，三份拷貝（Claude / Codex / 其他）各自漂移；沒裝技能、跑不了 bash（雲端、ChatGPT、手機）就做不了。CLI 和 MCP 只有底層的 `brief update`，沒有 `handoff`。
- **G2 下一個 AI 不會自己讀。** 開場沒有帶出 Brief；MCP 說明只說「resume 時讀」。結果：交接寫了也常沒人看。
- **G3 忘了就沒有。** 只有人叫 `/handoff` 才寫；沒有壓縮前、結束、閒置時的自動交接；Wenlan 已經讀對話，卻不拿來補交接。
- **G4 重複交接會疊加。** 去重只靠技能提示「Do not add an unchanged duplicate」、「Never fuzzy-match」；daemon 收到同樣文字的 `add` 照收。`operation_id` 只擋同一次重送，不擋同一個對話第二次交接。
- **G5 兩種交接格式。** 雲端對話寫自由格式 md，Mac 寫 Brief，互相看不到；專案資料夾的 137 份從沒進過 Wenlan。
- **G6 連不上時，雲端沒有排隊。** 本機 CLI 有 outbox；雲端寫不進 Mac 就直接失敗，沒有任何地方幫它排隊。
- **G7 失敗不明顯。** 雲端對話失敗多半只在交接檔裡寫一句「沒存進」，人不會知道。
- **G8 一次交接產出太多東西。** Brief、captures、敘事紀錄檔、`_status` 收據、頁面更新——對人來說只有「現在到哪」一個看得見；其他是 DX 負擔，也讓腳本長到 265 行。
- **G9 名字不一致。** 英文 app 叫 "Where this stands"，簡中「现在到哪」，指令叫 brief / handoff，技能卡叫「收工，留給下次」，雲端叫「交接檔」。
- **G10 同時交接同一條。** 有 version 衝突機制，但技能要 AI 自己 re-read 再對；自動交接時沒有 AI 可以「想一下」。

## 4. 目標體驗（簡潔、好懂、省心）

**人**
- 什麼都不用記。打開 Space：「現在到哪」一兩句、進行中每件一行、「上次交接：Codex，5 分鐘前」。（已上線，不加新面板）
- 交接成功不打擾；**沒存到才說一句**，而且說會怎麼補：「這次交接還沒存進 Wenlan，連上後會自動補。」

**AI 開工**
- 第一句之前就拿到這個 Space 的「現在到哪」（短，最多約 15 行），不用人問。

**AI 收工**
- 一個動作：`handoff`，每個入口同一個（CLI、MCP、技能只是叫它）。AI 只交「這次的結果」：做完什麼、決定什麼、卡在哪、下一步；比對、去重、合併由 Wenlan 做。
- 人叫 `/handoff` = 現在就做；不叫也會做（下一條）。

**忘了、累了、當掉**
- 對話快滿要壓縮前、對話結束時：自動交接。
- 晚上整理時：有在某個 Space 做事、但之後沒交接的對話，從對話本身補寫一次，標明「自動補寫」。

**重複、同時**
- 同一個對話交接幾次，都是更新它自己那幾條，不會多出來。
- 跟已有的一條文字相同（正規化後完全相同），就當成同一條，不新增。
- 兩個對話同時改同一條：留新的，舊的進歷史，不互相蓋掉，不問人。

**連不上**
- 本機：先存本機，Wenlan 開了就補（已有 outbox，沿用）。
- 雲端：先排隊在一個雲端連得到的地方（見 §6.5），Mac 連上就補。補的時候照上面的去重規則，不會重複。

## 5. 規則草案（每條一個測試）

| id | 規則 | 測試 |
|---|---|---|
| UX-HAND-1 | 每個 Space 只有一份「現在到哪」；app、CLI、MCP 看到的是同一份、同一個版本。 | 寫一次，三個入口讀到同一個 version 和文字（daemon 整合測試 + CLI/MCP 快照） |
| UX-HAND-2 | 交接是一個動作：CLI `wenlan handoff`、MCP `handoff`、HTTP `POST /api/handoff`；技能只叫它，不自己比對或合併。 | 三個入口送同一份內容，得到相同結果；技能檔不含 Brief 比對邏輯（lint） |
| UX-HAND-3 | AI 開工時自動拿到「現在到哪」：Claude Code 開場 hook 印出；其他 MCP 工具靠 server instructions 要求第一步讀 `brief`。Wenlan 沒開或 1 秒沒回就略過，不卡對話。 | hook 測試：有 Brief 時印出、沒開時 0 輸出且 <1.5 s |
| UX-HAND-4 | 對話壓縮前、結束時自動交接（Claude Code PreCompact / SessionEnd）。不需要模型時當下寫；需要模型時排到背景。 | hook 送出、daemon 收到並記為「自動」 |
| UX-HAND-5 | 晚上：最近一週在某 Space 做事、最後一次活動之後沒有交接的對話，補寫一次，標「自動補寫」，每個對話最多一次。 | 夜間測試：有交接的不補；沒交接的補一次；再跑不重複 |
| UX-HAND-6 | 同一個對話的第二次交接，更新它自己上次寫的條目，不新增。 | 同 session 交接兩次，條目數不變、內容更新 |
| UX-HAND-7 | 新增的一條與一條進行中的條目正規化後完全相同，就合併，不新增；不做模糊比對。 | 同文字 add 兩次只留一條；只差一字的兩條都留 |
| UX-HAND-8 | 同一條被兩邊同時改：新的生效，舊的進歷史；不丟、不問人。 | 併發測試：兩個舊 version 寫入，都在歷史裡 |
| UX-HAND-9 | 存不進去不能安靜失敗：排隊並在對話裡說一句；補送成功後不再說。 | 斷線測試：回覆含排隊說明；恢復後佇列清空 |
| UX-HAND-10 | 補送照 UX-HAND-6/7，重送不重複。 | 同一份排隊送兩次，結果相同 |
| UX-HAND-11 | 一個名字：中文「交接」「現在到哪」，英文 "Hand off" / "Where this stands"；指令名 `handoff` / `brief` 不變。 | 字串表檢查 |
| UX-HAND-12 | 團隊 Space：只有能寫這個 Space 的人與 AI 能交接；交接內容不帶私人 Space 的記憶文字。 | 權限測試（沿用 team_policy） |

## 6. DX 設計

### 6.1 `POST /api/handoff`
請求只描述「這次的結果」，不用先讀 Brief：

```json
{
  "space": "wenlan",            // 可省略：照現有 Space 解析
  "session": "codex:abc123",    // 有就帶；同 session 的條目屬於它
  "operation_id": "…",          // 可省略，daemon 生成；重送帶同一個
  "summary": "一兩句：現在到哪",
  "items": [
    {"text": "…", "state": "active|backlog|done", "gate": "等 Qi-Xuan 說合", "id": "可選：已知條目 id"}
  ],
  "auto": false                 // 自動交接為 true
}
```

daemon 依序：解析 Space → 有 `id` 的照 id 改 → 沒 `id` 的先找同 session 上次寫的條目 → 再找正規化相同文字 → 都沒有才新增 → `done` 的關掉 → 寫歷史 → 回收據（新增/更新/關閉/衝突各幾條）。內部重用現有 `apply_brief_update_in_session`，不另建存放。

### 6.2 入口
- CLI：`wenlan handoff "summary" [--item …] [--file x.json]`；daemon 沒開 → outbox（沿用）。
- MCP：`handoff` 工具，參數同上；`update_brief` 保留給需要精確控制的人，說明改指向 `handoff`。
- 技能：三份拷貝改成從一份產生（或一份共用），只做兩件事：從對話整理出 summary/items、叫 `handoff`。頁面更新（現在的第 7 步）拆成獨立的「更新用過的頁」，不是交接的一部分。

### 6.3 開工帶出
- Claude Code：`SessionStart` hook 呼叫 `POST /api/brief`，印出短版（摘要 + 進行中前 N 條 + 卡在誰），1 秒逾時。
- 其他 MCP 工具：server instructions 改成「開始處理這個專案的工作前，先讀 `brief`」。
- Codex：先查 Codex 現在有沒有開場 hook（待驗證）；沒有就靠 MCP instructions + AGENTS.md 的 Wenlan 區塊一行。

### 6.4 自動交接
- Claude Code `PreCompact`、`SessionEnd` hook → `POST /api/handoff/auto {session, transcript_path}`，daemon 排背景工作從對話寫交接（重用 `conversation_intake` 的讀取與 `keep_work` 過濾，不讀第二遍）。
- 夜間：同一個 intake 掃描，找「有 Space、沒交接」的對話，補寫（UX-HAND-5）。
- 沒有模型（Steep 設成 On demand、沒有本機模型）時：留一個待做，下次開工的 AI 在帶出 Brief 時順便被要求補（類似 `session-pages.sh` 的作法）。

### 6.5 雲端連不上（不等個人雲端）
三個選項，請 Astra 一起看：
- **A. relay 排隊**：relay.wenlan.app 在 Mac 不在線時收下 `handoff` 寫入（加密、只收 handoff、限大小與數量、保留 7 天），Mac 連上即補送。優點：產品本身解決，所有雲端工具都受惠。風險：資料暫存在我們的伺服器；relay KV 額度（11-14 出過事）。
- **B. 專案資料夾 + 補匯入**：雲端對話照現在寫檔，但改成和 `handoff` 同一個 JSON 格式；Mac 連上時由一個工作把未匯入的補進去。只解決我們自己的 claude.ai 專案，不是產品功能；而且 Mac 看不到 `/mnt/project-files`，要由雲端那邊在 Mac 在線時推過去。
- **C. 只靠 UX-HAND-5**：雲端對話的紀錄 Wenlan 讀不到，所以這條補不到雲端。不夠。
建議：**A 為產品解**，B 當我們自己專案的過渡。

## 7. 簡化（G8）
對人只留一樣：「現在到哪」+ 它的歷史。
- 敘事紀錄檔 `~/.wenlan/sessions/*.md`：改由 daemon 從交接歷史產生（或不再產生），技能不再寫。
- `_status/<space>.md` 收據：保留（人可讀、不當依據），由 daemon 寫。
- captures：留在技能裡但不是交接的必要步驟。
目標：技能從 ~265 行降到 ~40 行。

## 8. 分批
1. **S1** `handoff` 動作（HTTP/CLI/MCP）+ 去重合併（UX-HAND-1/2/6/7/8/10）。技能改薄。
2. **S2** 開工自動帶出（UX-HAND-3）。最小、效果最大，可和 S1 並行。
3. **S3** 自動交接 + 夜間補寫（UX-HAND-4/5）。
4. **S4** 雲端排隊（UX-HAND-9，§6.5 A）。
5. **S5** 名字統一（UX-HAND-11）、團隊（UX-HAND-12）。

## 9. 待決定（給 Qi-Xuan）
1. §6.5 雲端排隊：A（relay 暫存）還是先只做 B？
2. 自動交接要不要在對話裡說一句「已交接」？建議：成功不說，只有失敗說。
3. 敘事紀錄檔要不要留？建議：不留，由歷史取代。

## 10. 不做
- 不改首頁「正在做」的來源（UX-HOME-6）。
- 不做模糊比對合併。
- 不等個人雲端。
