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
- **UX-HOME-5** `shipped` A thing with no new conversation and no Brief change
  for a week leaves "what you're working on". Wenlan does not guess whether a
  thing is done; each card has "Don't show", which takes it off home at once.
  Either way it is not deleted: search still finds it, and its next
  conversation brings it back. There is no cap: home lists six, and "See all"
  opens the rest in place, without a count.
  原文：選擇卡「显示，安静 2 周后离开」（2026-11-02 23:40）；「我很担心我设上限会出现什么问题」（2026-11-02 21:53）；選擇卡「安静 1 周 +「不用显示」」（2026-11-03 02:46）；「我有点担心会 over engineering…第一性原理就是UI UX」（2026-11-03 02:39）。

## Sorting conversations into things

- **UX-THING-1** `planned` Every conversation belongs to at least one thing.
  You can see which, and change it in one step. When a conversation fits no
  thing yet, the AI starts a new thing and names it without asking.
  原文：每段對話都歸到至少一件事，看得到歸到哪，一步就能改。選擇卡「自动开新的」（2026-11-01 00:03）：對話不屬於任何一件事時，AI 直接開一件新事並取名，不問你；名字和歸屬都能一步改。
  Wenlan reads the conversations itself, from the transcripts Claude Code and
  Codex keep on this computer, so nobody has to remember to tell it. Quick
  questions, small chores (confirming a merge, sending a file), tests and runs
  a program started are left out. Wenlan's everyday model reads each
  conversation; grouping them into things takes a stronger model, which sees
  only one-line labels: the cloud model when one is set, otherwise the
  person's own AI tool. A thing is a goal, not a step: a review or a check
  goes with the goal it served. The grouping model also writes the card's
  line on where the whole thing stands. Titles and lines follow the app's
  language. A space with nothing written to it for a month is not suggested.
  原文：「正在做的事」直接從你跟 AI 的對話來，不用你記得叫 AI 存（2026-11-02）；選擇卡「用你接的 AI 工具（建議）」（2026-11-03 05:52）；「可以的那做完這個吧」（2026-11-03 05:23）；看了第七版：「有比較像最近在做的事情了」「wenlan-app 老早就沒在用的 space。」（2026-11-03 07:02）
- **UX-THING-2** `planned` After you correct it once, the same case is not
  sorted wrong again.
  原文：改過一次，同樣的情況下次不再分錯。
- **UX-THING-3** `shipped` Merging two things is only ever a proposal; they
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
  one page, not two copies: short is the page's first paragraph, never a
  separately written summary. The choice sits in the page's ⋯ menu ("Summary
  view", 只看重点), not above the text, and a short page ends with a way to
  the full one.
  原文：「Beta的字眼可以先拿掉」「又有 short 跟 full 在放在类似的位置，那就导致说那个文章被往下推了很多」（2026-11-01 23:42）
- **UX-VIEW-5** `planned` A page's first paragraph is its point. The AI writes
  its pages that way and fixes its older pages when it next updates them. On
  your pages it only proposes a first sentence, which counts once you accept it.
  原文：「或著我們第一段全文就該是重點？」「這樣擷取第一段就不用重寫摘要」（2026-10-31 20:23–20:24）
  原文：讀的時候能選短或全文，是同一頁，不是兩份。「這些功能因為還不確定需求，可以用 Beta 啥的字眼」（2026-10-31）
- **UX-VIEW-3** `shipped` The first screen is only for people; the details for
  AI are one collapsed section.
  原文：第一屏只有給人看的，給 AI 的細節是一個收起來的展開。
- **UX-VIEW-4** `shipped` The mind map keeps today's rule: the AI proposes,
  and nothing counts until you accept it.
  原文：心智圖現在「AI 提議、你收下才算」的做法留著。
- **UX-VIEW-6** `planned` A thing's short text is a recap in a fixed order:
  where it stands and the next step (at most two sentences), what is decided,
  what is still open, then the background. A group with nothing in it is not
  shown. Each point is one sentence; each group shows its newest three and
  folds the rest. Opening a point shows the lines of conversation it came
  from. What an AI reads about a thing is a separate, fuller version; the
  person sees only the recap.
  原文：「肯定是要 structure 的嘛」「有点像 recap 这样的概念」（2026-11-01 17:34）「AI 跟人看的，我不确定是不是要一样的」（2026-11-01 18:43）
- **UX-VIEW-7** `planned` A thing's mind map takes its shape from the
  conversation, not from fixed groups. The AI picks the shape: working toward a
  decision or fixing a problem becomes questions, options, reasons, and what
  was decided, left open or dropped; exploring or learning becomes a topic
  tree; anything that fits neither becomes a topic tree. The set of shapes
  stays open, so a new one can be added without changing the others.
  原文：「心智图应该要表达的是整个对话的它的那个就是抽象的结构，或是帮助你思考的结构」「我觉得它应该是根据对话的形式去决定的」（2026-11-01 18:10）「总之就是保留对未来的那个弹性吧」「我希望它是大方向定好之后下面都可以」（2026-11-01 19:52）

## Who said each sentence

- **UX-WHO-1** `planned` Every sentence shows whether it is yours (including
  ones you approved) or the AI's: visible, but quiet.
  原文：每一句都分得出是你的（含你點頭的）還是 AI 的，看得出來但不吵。
- **UX-WHO-2** `planned` The AI updates the sentences it wrote that nobody has
  touched; each update is recorded and can be undone. It cannot change your
  sentences: it proposes, one sentence at a time, with accept or no under that
  sentence, never a whole-page card. Each page has one Lock switch, off by
  default; there is no "may edit directly" mode. The AI proposes a change to
  your sentence only when the facts changed: a source no longer supports it or
  it has gone out of date. It never proposes a rewording.
  原文：AI 改不了你的句子，只能提議；提議能一句一句收。（2026-10-31 16:11 選擇卡：AI 自己寫、沒人動過的句子直接更新，可還原；你寫的、改過的、收下的才問你；每頁一個「鎖定」。）選擇卡「只在事实变了时」（2026-10-31 21:44）：AI 只在來源和你的句子對不上、或已經過時時才對你的句子提建議；只是換個說法不提。「尽量不 review 就尽量不 review」。
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
- **UX-CHAT-2** `shipped` Nothing is said when the reply has nothing to do with
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
- **UX-CHAT-5** `planned` On screen a thing stays one thing: half done and
  quiet for days, you can still find it, open it and carry on, with its
  conversations and map. Behind the scenes it works like working memory: each
  night the AI steeps what that day's conversations settled, and what is still
  open, into long-term memory and the big map by itself, and when you come
  back it brings the updated related points back in. The app shows that it
  happened, quietly: home names the things steeped overnight beside its
  heading, and the thing's page says how many points were steeped, with Undo. Nobody is asked
  "Wrap up?" and nothing is marked done. Each move into the big map is
  recorded and can be undone. The AI asks you only when a point conflicts with
  something you wrote or decided.
  原文：「收尾？」一件事只問一次，你點頭的點才接進大圖。（2026-10-30 17:19 PT 定了：安靜幾天後問一次「收尾？」，算在一天 5 條裡，不猜。）選擇卡「对不上时才问」（2026-10-31 23:50）：AI 自己把整理好的點搬進大圖，可還原；只有跟你寫過、定過的東西對不上時才問你。「我觉得不一定只有我点头」。「如果是 AI 要自己收，我觉得要自己收」（2026-11-01 00:06）「就有点像 working memory 嘛…过一段时间没做，没有动作，它就自己收到图里面去，大图里面去，就更新了。然后呢，下一次你又有什么新的？跟就算跟这件事情是有关的话，它可能就有点像是在一个新的 working memory…然后它过一段时间，它又会回去更，就会自己回去更新的」（2026-11-01 00:08）「做到一半…他想要回去找到他的话，他要重开一个新的，这是不合理的」「他界面上面看到的样子，跟你实际上背后做的样子…不一定要全部都一致的」（2026-11-01 00:10）選擇卡「可以，讀一次再收」（2026-11-03 17:24）。「在睡觉的时候…会把东西给存进到长期记忆里面去…你视觉上它可以留在那里待一周，但是实际上呢，它背后的东西在晚上就已经处理好了」（2026-11-03 17:25）「能在界面上什么地方可以显示说，哦，他晚上的时候经过整理的，所以让用户感觉到价值，而不是哦默默做完就默默做完」（2026-11-03 17:48）「应该要更优雅的说法…我们之前一直在强调那个 steeping」（2026-11-03 17:59）
- **UX-CHAT-6** `shipped` The check's rules live in Wenlan itself, not in each
  tool's plugin, so every AI tool gets the same result.
  原文：檢查的規則寫在文瀾本身，不寫在各家外掛裡；換哪個 AI 工具結果都一樣。
- **UX-CHAT-7** `planned` The AI tool tells you in one quiet line, with a
  link that opens it in the Wenlan app, only when Wenlan did something you
  can't see from the chat but may want to look at:
  1. the conversation is first saved into a thing, or moves to another one
     ("Saved in Wenlan: Plan the Lisbon trip");
  2. the AI wrote a new page;
  3. the AI changed a page that already existed (once per page in a
     conversation);
  4. the AI wanted to change sentences you wrote, so a suggestion waits for
     you.

  Nothing is said otherwise: later saves, reading or searching Wenlan, and
  tidying in the background stay quiet. A reply has at most one such line;
  several of these join it ("Saved in Wenlan: Kyoto trip · wrote Gion stay").
  原文：「对话里面如果记一个也是可以」（2026-11-01 17:29）「聽起來可以，有辦法直接跳轉到 wenlan app?」（2026-11-01 20:11）「我觉得它其实是感觉是需要的…什么样的情况下…它是需要在 AI 工具那里面跟我们知会一声的」（2026-11-01 21:15）；選擇卡「四种都说」（2026-11-01 21:24）

## The big map

- **UX-GRAPH-1** `shipped` The same point is in the same place every time you
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
  original words from conversations. The groups come in this order: what you're
  working on, pages, points on the map, the words said in conversations, then
  topics. Memories, the AI's own notes, have no group of their own.
  原文：一個框，結果包含頁、圖上的點和對話裡的原話。選擇卡「一个搜寻框找全部」（2026-11-01 22:19）；分組順序「正在做的事 → 笔记 → 图上的点 → 原话」、主題留在最後，「其他都可以」（2026-11-01 23:47）
- **UX-FIND-2** `planned` Opening a source shows that sentence with a little
  of what came before and after, and where it is used. You can expand it to
  the whole original. Imported originals stay backstage: they are not in page
  lists or on home. The whole conversation reads like the AI tool it came from:
  the person's words in a light bubble on the right, the AI's replies across
  the width; the short preview stays one column with who spoke.
  原文：點出處只顯示那一句和用在哪，文瀾裡不顯示整段對話。「C 啊，不过能看到前后一点点的话会更好，然后也能展开看全部」（2026-10-31 16:54）「一般这种对话怎么渲染会更适合一些？就像现在这样在左边，还是说要左右左右这样子的？」（2026-11-02 16:03）「2. 同意」（2026-11-02 16:47）
- **UX-FIND-3** `shipped` There is "open in the original AI tool".
  原文：有「在原本的 AI 工具打開」。
- **UX-FIND-4** `planned` The left side shows Search, Home, Wiki, Spaces, the
  graph and topics, with no More menu: there are few enough to show them all.
  Activity stays at the bottom. There is no Memories page and no
  Sources page: a source is what a sentence cites, so a source shows which
  pages cite it, and the list of added files and links opens from Settings.
  When a source can't be reached (moved, or its drive unplugged), Activity says
  so in its own place at the bottom left, not on Home, and opens that source.
  原文：「以及要把左邊的導覽，只留下重要的，剩下的可以不用特別的 tab」（2026-11-01 22:20）「我确定是、不会特别需要有一个记忆的页面」（2026-11-01 23:35）「活动还是要在的……它就在左下角」（2026-11-01 23:46）「那你说你有什么情况下会特别需要到一个来源的页面，导览页面里面去管理那些东西？感觉机会并不是很大。」（2026-11-02 16:02）「1. 同意」（2026-11-02 16:47）選擇卡「左下角「活动」」（2026-11-02 18:11）「因為現在選項沒有很多，全放出來也還好」（2026-11-02 19:38）

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
