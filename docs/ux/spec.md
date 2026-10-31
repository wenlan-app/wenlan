# Wenlan UX spec

This file is the agreed user experience. Each rule has an id. A rule marked
`shipped` is enforced by at least one test whose name contains the id
(`UX-HOME-1` in a TypeScript test name, `ux_home_1` in a Rust test name).
`src/uxSpec.test.ts` fails when a shipped rule has no test, when a test names an
id this file does not define, or when an id appears twice.

How to change it:

- Only Qi-Xuan changes what a rule says. An agent may flip `planned` to
  `shipped` in the same change that adds the rule's test.
- The text wins over the design boards. Boards illustrate; they do not decide.
- A change that alters what the app shows needs real screenshots and Qi-Xuan's
  OK before merge.
- When code and a shipped rule disagree, fix the code or ask; never reword the
  rule to match the code.

Each rule keeps the original Traditional Chinese line it was agreed in. Source
documents: the spec doc (claude.ai artifact G5qFLYM31zsH8iKHXBQPHW) and the
design boards (claude.ai artifact Xcq7Uh2EAmoyGHeL3JdmFU), both from
2026-10-31.

## Words

- **A thing (一件事)**: one line of work, such as "the new tagline" or "the
  Product Hunt launch". It is smaller than a Space: a Space is a project and
  follows a folder or repository, and one Space holds several things. A thing
  can span Spaces. Until things have their own record, each active item of a
  Space Brief stands for one thing.
- **Original (原文)**: text that came in from outside, such as an imported
  chat, a web page or a PDF. It is an ordinary page whose author is 原文. The
  AI can't edit it, and it is not yours, so it never counts as decided.
  原文：（2026-10-31 04:04 定了）外面進來的原文是一般的頁，作者記成「原文」，AI 不能改。
- **A thing's mind map**: one map per thing. It is the map you see beside a
  conversation (the working memory, built from the chat and the pages it
  uses) and the same map is that thing's part of the big map. Points not yet
  tidied are dashed; only tidied points join long-term memory. Beside a
  conversation the whole map shows, and the points this conversation used or
  added are lit; the rest are faded.
  原文：「大图里面的那张小图，的这件事，它是同一张」「比较像是虚线，就是它还没有真的被接进那个 long term memory 里面」「它会经过整理什么的，才会存进长期记忆里的」（2026-10-31）；選擇卡「共用一张，这次的点亮起来」（2026-10-31 16:50）
- **The big map**: everything connected. Pages are in it as topics, not as
  work in progress.
  原文：「graph 里面也会有笔记。笔记的话，它就不会是跟着一个事，它就会是一个主题或是一个事情，而不会是一个在进行中的东西」（2026-10-31）
- **Yours**: a sentence you wrote, edited or accepted. Having seen it does not
  make it yours. Only yours counts as decided.
- **Proposal**: a change the AI suggests to one of your sentences. It lands
  only when you accept it.

## Home

- **UX-HOME-1** `planned` The first screen is "what you're working on". Each
  thing gets one line saying where it stands; opening it goes to that thing.
  原文：打開 app 的第一屏是「正在做的事」，每件一句做到哪，點進去就是那件事。
- **UX-HOME-2** `planned` Home shows no counts: no number of sources, pages,
  archived pages or conversations.
  原文：首頁沒有任何計數：來源數、頁數、封存數、幾段對話都不放。
- **UX-HOME-3** `planned` Below "what you're working on" are the pages you
  edited last. Home has no thumbnail of the mind map or the big map; maps stay
  inside pages. "What you're working on" shows only when there is something.
  The recent list holds pages you edited and pages the AI newly wrote (quietly
  marked as the AI's); the AI's small edits to old pages don't put them there.
  原文（2026-10-31 選擇卡）：正在做的事 + 最近編輯，不放小圖。選擇卡「你改的 + AI 新写的」（2026-10-31 17:16）
- **UX-HOME-4** `planned` Check results and source updates are not on home.
  原文：檢查結果和來源更新不放首頁。

## Sorting conversations into things

- **UX-THING-1** `planned` Every conversation belongs to at least one thing.
  You can see which, and change it in one step.
  原文：每段對話都歸到至少一件事，看得到歸到哪，一步就能改。
- **UX-THING-2** `planned` After you correct it once, the same case is not
  sorted wrong again.
  原文：改過一次，同樣的情況下次不再分錯。
- **UX-THING-3** `planned` Merging two things is only ever a proposal; they
  merge when you press it.
  原文：合併只會是提議，你按了才合。

## A thing's page: short text, mind map, any shape you want

- **UX-VIEW-1** `planned` Short text and mind map are two views of the same
  thing. Switching does not change pages, and the last view you used is
  remembered. There is no third view for now: an HTML page an AI tool draws in
  chat is saved as its own page (UX-ART-1 to 3). Add a third view only once
  people save such pages for their things often.
  原文：短文、心智圖、你要的其他樣子是同一件事的三個看法，切換不換頁，記得你上次用哪個。「因为我们还没有很明确的用户需求，这样的情境下做的选择会是哪一个就选哪一个」→ 先不做第三種（2026-10-31 19:10）
- **UX-VIEW-2** `shipped` While reading you can pick short or full text. It is
  one page, not two copies. The switch is marked Beta.
  原文：讀的時候能選短或全文，是同一頁，不是兩份。「這些功能因為還不確定需求，可以用 Beta 啥的字眼」（2026-10-31）
- **UX-VIEW-3** `shipped` The first screen is only for people; the details for
  AI are one collapsed section.
  原文：第一屏只有給人看的，給 AI 的細節是一個收起來的展開。
- **UX-VIEW-4** `shipped` The mind map keeps today's rule: the AI proposes,
  and nothing counts until you accept it.
  原文：心智圖現在「AI 提議、你收下才算」的做法留著。

## Who said each sentence

- **UX-WHO-1** `planned` Every sentence shows whether it is yours (including
  ones you approved) or the AI's: visible, but quiet.
  原文：每一句都分得出是你的（含你點頭的）還是 AI 的，看得出來但不吵。
- **UX-WHO-2** `planned` The AI updates the sentences it wrote that nobody has
  touched; each update is recorded and can be undone. It cannot change your
  sentences: it proposes, one sentence at a time, with accept or no under that
  sentence, never a whole-page card. Each page has one Lock switch, off by
  default; there is no "may edit directly" mode.
  原文：AI 改不了你的句子，只能提議；提議能一句一句收。（2026-10-31 16:11 選擇卡：AI 自己寫、沒人動過的句子直接更新，可還原；你寫的、改過的、收下的才問你；每頁一個「鎖定」。）
- **UX-WHO-3** `planned` Every sentence has "not right": fix it yourself or ask
  the AI; the AI's fix is still a proposal.
  原文：每一句都能按「不對」：你自己改，或讓 AI 改，AI 改的仍然是提議。
- **UX-WHO-4** `planned` A sentence that can't be backed up is marked next to
  that sentence, not in a banner at the top. One press asks the AI to recheck
  only that sentence.
  原文：靠不住的句子標在那一句旁邊，不是頁頂橫幅；按一下，AI 只重查那一句。
- **UX-WHO-5** `shipped` An unverified source can't be hidden and can't look
  verified.
  原文：現有規則不變：沒驗證的出處不能藏，也不能看起來像驗證過。

## While you chat (in your own AI tool)

- **UX-CHAT-1** `planned` When you or the AI say something that contradicts an
  earlier decision or the record, a one-line reminder appears in the same turn,
  with the date and the source, and it can be opened. Both your words and the
  AI's are checked. It is a Beta setting, off by default.
  原文：AI 說了跟以前定的相反的話，同一輪就出現一句提醒，帶日期和出處，點得開。「這些功能因為還不確定需求，可以用 Beta 啥的字眼」（2026-10-31；檢查改成要你打開才有）；選擇卡「你和 AI 说的都看」（2026-10-31 16:51）
- **UX-CHAT-2** `planned` Nothing is said when the reply has nothing to do with
  earlier decisions. If Wenlan is closed or does not answer within one second,
  the check is skipped and the chat is not held up. At most five reminders a
  day.
  原文：跟以前無關的話不出聲；文瀾沒開或一秒內沒回就跳過，不卡對話；一天最多 5 條。
- **UX-CHAT-3** `planned` In tools that can show a small window, the thing's
  mind map is visible and follows the conversation within 30 seconds; every
  point opens that page in Wenlan.
  原文：能顯示小視窗的工具裡，看得到這件事的心智圖，30 秒內跟著對話變；每個點都能點回文瀾那一頁。
- **UX-CHAT-4** `planned` Tools that can't show a window (for example the
  Claude Code terminal) get the same mind map as UX-CHAT-3, this
  conversation's working memory built from the conversation and the source
  pages it uses, as a small text tree with an "open in Wenlan" link. A single
  page's own map comes only when you ask for that page.
  原文：不能顯示小視窗的工具（例如 Claude Code 命令列）給一個小的文字樹，加「在文瀾打開」。
  「我是认为是读的是一个工作状态上面的一个心智图，因为这跟这个对话有关嘛」「笔记的那个心智图…除非是有必要想去看」「把不同的就是来源笔记啊啥的，然后把它读到这个就是 working memory 里面的一个心智图」（2026-10-31）
- **UX-CHAT-5** `planned` "Wrap up?" is asked once per thing, after the thing
  has been quiet for a few days; the AI never guesses that it is done. The
  question counts toward the five reminders a day. Only points you approve
  join the big map.
  原文：「收尾？」一件事只問一次，你點頭的點才接進大圖。（2026-10-30 17:19 PT 定了：安靜幾天後問一次「收尾？」，算在一天 5 條裡，不猜。）
- **UX-CHAT-6** `planned` The check's rules live in Wenlan itself, not in each
  tool's plugin, so every AI tool gets the same result.
  原文：檢查的規則寫在文瀾本身，不寫在各家外掛裡；換哪個 AI 工具結果都一樣。

## The big map

- **UX-GRAPH-1** `planned` The same point is in the same place every time you
  open it. Rearranging asks you first and can be undone.
  原文：同一個點，兩次打開位置一樣；要重排先問你，能退回。
- **UX-GRAPH-2** `planned` Three zoom levels (this thing, nearby, everything)
  with continuous motion; points do not jump.
  原文：拉遠三段：這件事 → 附近 → 全部，畫面連續，點不跳位置。
- **UX-GRAPH-3** `planned` Drag, delete, add points, connect and lasso, and it
  is all still there after reopening. What the AI adds is dashed until you
  accept it.
  原文：拖、刪、加點、連線、圈選都可以，重開還在；AI 加的是虛線，你收下才變實線。
- **UX-GRAPH-4** `planned` Lasso three points and press "take to chat": the AI
  tool receives a summary with sources, including the lines you drew.
  原文：圈 3 個點按「帶去對話」，AI 工具收到摘要和出處，連你連的線一起帶上。

## Finding things

- **UX-FIND-1** `planned` One box; results include pages, points on the map and
  original words from conversations.
  原文：一個框，結果包含頁、圖上的點和對話裡的原話。
- **UX-FIND-2** `planned` Opening a source shows that sentence with a little
  of what came before and after, and where it is used. You can expand it to
  the whole original. Imported originals stay backstage: they are not in page
  lists or on home.
  原文：點出處只顯示那一句和用在哪，文瀾裡不顯示整段對話。「C 啊，不过能看到前后一点点的话会更好，然后也能展开看全部」（2026-10-31 16:54）
- **UX-FIND-3** `planned` There is "open in the original AI tool".
  原文：有「在原本的 AI 工具打開」。

## What AI tools made (HTML, images)

- **UX-ART-1** `planned` Saved HTML opens in Wenlan looking exactly like the
  original.
  原文：存進來的 HTML 在文瀾打開，跟原本一樣。
- **UX-ART-2** `planned` It is a page with its source: which conversation, and
  when.
  原文：它是一頁，帶出處：哪段對話、什麼時候。
- **UX-ART-3** `planned` The AI never rewrites it. If the facts change later, it
  only adds a note.
  原文：AI 不改寫它；後來事實變了，只加註記。

## Adding to Wenlan

- **UX-ADD-1** `planned` There is one "Add to Wenlan" in the app; the shortcut,
  the menu bar and Settings all open the same one.
  原文：app 裡只有一個「加進文瀾」，其他地方（快捷鍵、選單列、設定）都打開同一個。
- **UX-ADD-2** `planned` Files, folders, chat exports and links can all be
  dropped in without first asking what kind they are.
  原文：檔案、資料夾、對話匯出、連結拖進去都收，不先問你是哪一類。
- **UX-ADD-3** `planned` An Obsidian folder that has never used Wenlan works
  when connected, without installing a plugin.
  原文：接上一個沒用過文瀾的 Obsidian 資料夾，不裝外掛就能用。

## History

- **UX-HIST-1** `planned` One history per page, with three kinds of change in
  one list: what you changed, what the AI changed, and a source it cites that
  changed. Renames, moves and locking are not listed.
  原文：一頁一本歷史，三種變動在同一個列表。選擇卡「你改的、AI 改的、来源变了」（2026-10-31 17:34）
- **UX-HIST-2** `planned` Every entry opens to show what changed.
  原文：每筆都能打開看改了什麼。
