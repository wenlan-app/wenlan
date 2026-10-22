// SPDX-License-Identifier: AGPL-3.0-only
//
// Locale literals for the background-activity surfaces: the toolbar Activity
// button, its summary popover, and the Now section on the Activity page.
// Imported by src/i18n/resources.ts into the `activityStatus` namespace of
// every locale. NOT `activity`: that namespace already exists and belongs to
// the Activity feed (actions, groups, filters, relative times). A second
// `activity:` key in the same object literal silently REPLACES the first, so
// reusing the name would delete the feed's copy with no error at the wiring
// site, with only a wall of t() type errors in ActivityFeed.tsx. Relative times
// come from the existing `activity.relative.*`; there is one such vocabulary. The three objects must keep identical key sets (parity is enforced
// by src/i18n/resources.test.ts).
//
// Register note: the spec drafts said "on this Mac". Wenlan ships on Windows
// and Linux too, so the lane copy says "this machine", matching the wording
// already used by setup.privacyTitle and setup.intelligence.deviceNote.
// Vendor names (Anthropic, OpenAI) stay literal in every locale.
//
// No Retry copy. The spec asks for a Retry that reruns only the failed steps,
// and no such capability exists: failed enrichment rows are re-selected by the
// scheduler automatically until ENRICHMENT_MAX_ATTEMPTS and then become
// `abandoned`, which is terminal with no reset path (crates/wenlan-core/src/
// post_ingest.rs:107-119). There is no daemon route and no Tauri command for
// it. Shipping a button that cannot do what it says would be worse than
// leaving it out, so Retry waits for the daemon work.

export const enActivityStatus = {
  title: "Activity",
  // ── Tier 0, the toolbar Activity button ────────────────────────────────
  state: {
    up_to_date: "Up to date",
    off: "On demand",
    organizing: "Steeping",
    // Steeping that cannot run yet (ActivityState waiting_for_idle).
    waiting_for_idle: "Waiting for a quiet moment",
    blocked: "Blocked",
    failed: "Failed",
  },
  statusLabel: "Background activity",
  // The toolbar button's accessible name: its visible word plus the state,
  // which sighted users read from the dot and the tooltip.
  buttonLabel: "Activity, {{state}}",

  // ── Tier 1, the summary popover ───────────────────────────────────────
  headline: {
    up_to_date: "Everything you have given Wenlan has steeped.",
    off: "Steeping runs on demand. Your AI tool steeps as it uses Wenlan.",
    organizing: "Wenlan is steeping what you have given it.",
    // Pairs with lastActivity: an old time under this line is expected, so it
    // must not claim anything is running. It names every reason the scheduler
    // holds work, so a long wait while the user is away (low memory, a warm
    // computer) is still explained.
    waiting_for_idle: "Wenlan will keep steeping when your computer is quiet. It holds off while you use it, or while it is busy, low on memory or running hot.",
    blocked: "Some steeping is waiting on you.",
  },
  asset: {
    memories: "Memories",
    entities: "Entities",
    pages: "Pages",
  },
  summary: {
    pagesCurrent: "All pages are current.",
    memoriesWaiting: "{{count}} awaiting summaries · searchable now",
    entitiesWaiting_one: "{{count}} memory awaiting a scan",
    entitiesWaiting_other: "{{count}} memories awaiting a scan",
  },
  rowAsset: { memories: "Memories", entities: "Entities", pages: "Pages" },
  // On demand: nothing runs in the background, so a row says what is
  // waiting for the user's AI tool instead of reading as stalled.
  assetOnDemand: {
    memories_one: "{{count}} stored, searchable now",
    memories_other: "{{count}} stored, all searchable now",
    entities_one: "{{count}} memory not yet scanned for entities",
    entities_other: "{{count}} memories not yet scanned for entities",
    pages_one: "{{count}} page still needs steeping",
    pages_other: "{{count}} pages still need steeping",
  },
  onDemandFinish: "Your AI tool handles these as it uses them. To finish all now, run /wenlan steep.",
  assetDone: {
    memories_one: "{{count}} stored, summarized and linked",
    memories_other: "{{count}} stored, all summarized and linked",
    entities: "{{count}} found, {{confirmed}} confirmed in the Wiki",
    pages_one: "{{count}} page written, current",
    pages_other: "{{count}} pages written, all current",
  },
  assetEmpty: {
    memories: "Nothing stored yet",
    entities: "Nothing found yet",
    pages: "No pages yet",
  },
  // Every number names its unit. Entities' running and blocked counts are
  // MEMORIES scanned for entities, not entities: one memory can name several
  // entities or none, so "8" beside "Entities" would be a wrong number.
  assetRunning: {
    memories: "{{done}} of {{total}} summarized and linked",
    entities_one: "{{done}} of {{total}} memory scanned for entities",
    entities_other: "{{done}} of {{total}} memories scanned for entities",
    pages: "{{done}} of {{total}} pages current, the rest updating",
  },
  // Rows say only how many are waiting, in their own unit. The cause and the
  // fix are said ONCE, per missing model, by blockedCause: Memories and
  // Entities share the everyday model, so a per-row cause printed the same
  // sentence twice.
  assetBlockedNoModel: {
    memories: "{{count}} not yet summarized. All are searchable now.",
    entities_one: "{{count}} memory not yet scanned for entities",
    entities_other: "{{count}} memories not yet scanned for entities",
    pages_one: "{{count}} page waiting to be updated",
    pages_other: "{{count}} pages waiting to be updated",
  },
  // The popover opens on this, with turnOnModel beside it, so it names what
  // stopped and why; the button carries the fix. turnOnModel repeats Home's
  // empty-state button word for word: one action, one name.
  blockedCause: {
    everyday: "Steeping is paused because no model is set up. Your AI tools can still organize memories.",
    synthesis: "Pages update when your AI tools write them, because no writing model is set up.",
  },
  // A model was chosen but is not serving: still loading, or its server is
  // down. The fix is the same page, so turnOnModel still applies.
  blockedCauseUnavailable: {
    everyday: "Steeping is paused because its model is unavailable.",
    synthesis: "Page updates are paused because their model is unavailable.",
  },
  turnOnModel: "Turn on a model",
  assetBlockedFailed: {
    memories_one:
      "{{count}} of {{total}} blocked: summarizing failed. All {{total}} are still searchable.",
    memories_other:
      "{{count}} of {{total}} blocked: summarizing failed. All {{total}} are still searchable.",
    entities:
      "{{count}} of {{total}} memories blocked: entity detection failed. Your memories are unaffected.",
    pages:
      "{{count}} of {{total}} pages blocked: page writing failed. Your sources are unaffected.",
  },
  openActivity: "See all activity",
  jobSeparator: " and ",
  lastActivity: "Last activity {{time}}",
  // Pairs with lastActivity, so it reports when work last RAN. It must not say
  // there is no background work: this string shows exactly when nothing has
  // finished, which is also when a blocked library has the most work waiting.
  neverActive: "Nothing has run yet",

  // ── Tier 2, the Now section on the Activity page ───────────────────────
  nowTitle: "Steeping",
  nowLoading: "Checking current work…",
  nowError: "Could not read current work.",
  nowIdle: "No work is currently in progress.",
  nowOff: "Steeping runs on demand.",
  nowRunning: "Steeping is running.",
  nowBlocked: "Steeping is blocked.",
  nowWaiting: "Waiting for your computer to be idle.",
  nowPaused: "Steeping is paused.",
  nowUnknown: "Current work status is temporarily unavailable.",
  eventsLoading: "Loading recent activity…",
  eventsError: "Could not load recent activity.",
  readAgain: "Read again",
  runningStep: {
    store: "Storing memories.",
    summarize: "Organizing memory summaries.",
    link: "Linking related memories.",
    detect: "Identifying topics in memories.",
    confirm: "Confirming topics.",
    write: "Updating pages.",
  },
  failedStep: {
    store_one: "{{count}} memory could not be stored.",
    store_other: "{{count}} memories could not be stored.",
    summarize_one: "{{count}} memory could not be summarized.",
    summarize_other: "{{count}} memories could not be summarized.",
    link_one: "{{count}} memory could not be linked.",
    link_other: "{{count}} memories could not be linked.",
    detect_one: "{{count}} memory could not be scanned for topics.",
    detect_other: "{{count}} memories could not be scanned for topics.",
    confirm_one: "{{count}} entity could not be confirmed.",
    confirm_other: "{{count}} entities could not be confirmed.",
    write_one: "{{count}} page could not be updated.",
    write_other: "{{count}} pages could not be updated.",
  },
  failedUnknown: "Some background work has failed.",
  showSteps: "Show steps",
  hideSteps: "Hide steps",
  step: {
    store: "Store",
    summarize: "Summarize",
    link: "Link",
    detect: "Detect",
    confirm: "Confirm",
    write: "Write",
  },
  stepValue: {
    store: "saved and searchable right away",
    summarize: "summary and tags so recall is accurate",
    link: "connected to related memories and entities",
    detect: "people, projects and tools you mention",
    confirm: "earned enough substance to appear in the Wiki",
    write: "pages written and refreshed from your sources",
  },
  stepState: {
    idle: "Idle",
    running: "Running",
    blocked: "Blocked",
    waiting: "Waiting",
    unknown: "Status unavailable",
  },
  stepComplete: "Complete",
  compactStepCount: {
    memories_one: "{{done}} / {{count}} memory",
    memories_other: "{{done}} / {{count}} memories",
    entities_one: "{{done}} / {{count}} entity",
    entities_other: "{{done}} / {{count}} entities",
    pages_one: "{{done}} / {{count}} page",
    pages_other: "{{done}} / {{count}} pages",
  },
  moreDetails: "Details",
  processingDetails: "Models and privacy",
  // Per step, in that step's unit: Detect counts memories, Confirm entities.
  stepCount: {
    memories_one: "{{done}} of {{count}} memory",
    memories_other: "{{done}} of {{count}} memories",
    entities_one: "{{done}} of {{count}} entity",
    entities_other: "{{done}} of {{count}} entities",
    pages_one: "{{done}} of {{count}} page",
    pages_other: "{{done}} of {{count}} pages",
  },
  lane: {
    on_device: "on this machine",
    external: "on your local server",
    anthropic: "on Anthropic",
    basic: "built in, no model",
    none: "no model in use",
  },
  // One sentence per route. A route with no model prints its lane alone:
  // interpolating a "no model" placeholder AND the lane read "no model in use
  // built in, no model".
  route: "{{job}}: {{model}} {{lane}}.",
  routeNoModel: "{{job}}: {{lane}}.",
  jobTitle: {
    everyday: "Everyday work",
    synthesis: "Page writing",
  },
  job: {
    everyday: "everyday work",
    synthesis: "page writing",
  },
  openSettings: "Open settings",
  trustLocal: "Runs on this machine. Nothing leaves your device.",
  trustCloud:
    "{{jobs}} runs on {{vendor}}; the text for that step leaves your device. Everything else stays on this machine.",
  // Open refinement suggestions. Ready ones are what the review queue lists;
  // the rest are open but not listed yet, and some never move on their own,
  // so neither line claims the library is waiting on anything.
  suggestions: {
    title: "Suggestions",
    ready_one: "{{count}} waiting for your review",
    ready_other: "{{count}} waiting for your review",
    notReady_one: "{{count}} not yet ready for review",
    notReady_other: "{{count}} not yet ready for review",
  },

  // ── Import handoff ────────────────────────────────────────────────────
  importHandoff_one:
    "{{count}} memory stored and searchable now. Wenlan keeps steeping it in the background. See further progress on Activity.",
  importHandoff_other:
    "{{count}} memories stored and searchable now. Wenlan keeps steeping them in the background. See further progress on Activity.",
  importHandoffOnDemand_one:
    "{{count}} memory stored and searchable now. Your AI tool steeps it as it uses Wenlan. See progress on Activity.",
  importHandoffOnDemand_other:
    "{{count}} memories stored and searchable now. Your AI tool steeps them as it uses Wenlan. See progress on Activity.",

  // ── Settings, Appearance ──────────────────────────────────────────────
  layoutSetting: "Activity summary placement",
  layoutSettingHelp:
    "Where the Now summary sits on the Activity page. Saved for this device.",
  layout: {
    rail: "Side rail",
    card: "Card above the feed",
    timeline: "In the feed",
  },
};

export const hansActivityStatus = {
  title: "动态",
  state: {
    up_to_date: "已就绪",
    off: "随叫",
    organizing: "沉淀中",
    // Not 暂停: that word is Blocked in this locale.
    waiting_for_idle: "等电脑空闲",
    blocked: "已暂停",
    failed: "失败",
  },
  statusLabel: "后台工作",
  buttonLabel: "活动，{{state}}",
  headline: {
    up_to_date: "你交给文澜的内容都已沉淀完毕。",
    off: "沉淀随叫进行，你的 AI 工具用文澜时会顺手整理。",
    organizing: "文澜正在沉淀你交给它的内容。",
    waiting_for_idle: "文澜会在电脑空闲时继续沉淀。你使用电脑，或电脑忙碌、内存不足、温度过高时，它会先等一等。",
    blocked: "部分沉淀工作在等你处理。",
  },
  asset: {
    memories: "记忆",
    entities: "实体",
    pages: "笔记",
  },
  summary: {
    pagesCurrent: "所有笔记均已更新。",
    memoriesWaiting: "{{count}} 条待摘要，可直接搜索",
    entitiesWaiting_one: "{{count}} 条记忆待识别主题",
    entitiesWaiting_other: "{{count}} 条记忆待识别主题",
  },
  rowAsset: { memories: "记忆", entities: "实体", pages: "笔记" },
  assetOnDemand: {
    memories_one: "已存 {{count}} 条，都可以搜索",
    memories_other: "已存 {{count}} 条，都可以搜索",
    entities_one: "{{count}} 条记忆还没识别实体",
    entities_other: "{{count}} 条记忆还没识别实体",
    pages_one: "{{count}} 篇笔记还没整理",
    pages_other: "{{count}} 篇笔记还没整理",
  },
  onDemandFinish: "你的 AI 工具用到时会顺手做；想一次做完，输入 /wenlan steep。",
  assetDone: {
    memories_one: "已存 {{count}} 条，全部已生成摘要并建立关联",
    memories_other: "已存 {{count}} 条，全部已生成摘要并建立关联",
    entities: "发现 {{count}} 个，其中 {{confirmed}} 个已收入 Wiki",
    pages_one: "已写 {{count}} 篇笔记，全部为最新",
    pages_other: "已写 {{count}} 篇笔记，全部为最新",
  },
  assetEmpty: {
    memories: "还没有存入内容",
    entities: "还没有发现实体",
    pages: "还没有笔记",
  },
  assetRunning: {
    memories: "{{total}} 条中已完成 {{done}} 条的摘要与关联",
    entities_one: "{{total}} 条记忆中已有 {{done}} 条完成实体扫描",
    entities_other: "{{total}} 条记忆中已有 {{done}} 条完成实体扫描",
    pages: "{{total}} 篇笔记中 {{done}} 篇为最新，其余正在更新",
  },
  assetBlockedNoModel: {
    memories: "{{count}} 条尚未生成摘要。全部已可搜索。",
    entities_one: "{{count}} 条记忆尚未扫描实体",
    entities_other: "{{count}} 条记忆尚未扫描实体",
    pages_one: "{{count}} 篇笔记等待更新",
    pages_other: "{{count}} 篇笔记等待更新",
  },
  blockedCause: {
    everyday: "沉淀已暂停，因为尚未设置模型。你的 AI 工具仍可整理记忆。",
    synthesis: "尚未设置撰写模型，所以笔记会在你的 AI 工具撰写时更新。",
  },
  blockedCauseUnavailable: {
    everyday: "沉淀已暂停，因为模型暂时无法使用。",
    synthesis: "笔记更新已暂停，因为撰写模型暂时无法使用。",
  },
  turnOnModel: "启用模型",
  assetBlockedFailed: {
    memories_one: "{{total}} 条中有 {{count}} 条暂停：生成摘要失败。全部 {{total}} 条仍可搜索。",
    memories_other: "{{total}} 条中有 {{count}} 条暂停：生成摘要失败。全部 {{total}} 条仍可搜索。",
    entities: "{{total}} 条记忆中有 {{count}} 条暂停：实体识别失败。你的记忆不受影响。",
    pages: "{{total}} 篇笔记中有 {{count}} 篇暂停：写笔记失败。你的来源不受影响。",
  },
  openActivity: "查看全部活动",
  jobSeparator: "和",
  lastActivity: "最近活动于{{time}}",
  neverActive: "尚未运行过",
  nowTitle: "沉淀",
  nowLoading: "正在查看当前工作…",
  nowError: "无法读取当前工作。",
  nowIdle: "目前没有进行中的工作。",
  nowOff: "沉淀随叫进行。",
  nowRunning: "沉淀正在进行。",
  nowBlocked: "沉淀受阻。",
  nowWaiting: "等待电脑空闲时继续。",
  nowPaused: "沉淀已暂停。",
  nowUnknown: "目前工作状态暂时无法读取。",
  eventsLoading: "正在加载最近动态…",
  eventsError: "无法加载最近动态。",
  readAgain: "重新读取",
  runningStep: {
    store: "正在存入记忆。",
    summarize: "正在整理记忆摘要。",
    link: "正在关联相关记忆。",
    detect: "正在识别记忆中的主题。",
    confirm: "正在确认主题。",
    write: "正在更新笔记。",
  },
  failedStep: {
    store_one: "有 {{count}} 条记忆未能存入。",
    store_other: "有 {{count}} 条记忆未能存入。",
    summarize_one: "有 {{count}} 条记忆未能生成摘要。",
    summarize_other: "有 {{count}} 条记忆未能生成摘要。",
    link_one: "有 {{count}} 条记忆未能关联相关内容。",
    link_other: "有 {{count}} 条记忆未能关联相关内容。",
    detect_one: "有 {{count}} 条记忆未能完成主题识别。",
    detect_other: "有 {{count}} 条记忆未能完成主题识别。",
    confirm_one: "有 {{count}} 个实体未能确认。",
    confirm_other: "有 {{count}} 个实体未能确认。",
    write_one: "有 {{count}} 篇笔记未能更新。",
    write_other: "有 {{count}} 篇笔记未能更新。",
  },
  failedUnknown: "部分后台工作失败。",
  showSteps: "展开步骤",
  hideSteps: "收起步骤",
  step: {
    store: "存入",
    summarize: "摘要",
    link: "关联",
    detect: "识别",
    confirm: "确认",
    write: "撰写",
  },
  stepValue: {
    store: "存入后立即可搜索",
    summarize: "生成摘要与标签，让回忆更准确",
    link: "与相关记忆和实体建立关联",
    detect: "你提到的人、项目和工具",
    confirm: "内容足够充实，可收入 Wiki",
    write: "依据你的来源撰写并更新笔记",
  },
  stepState: {
    idle: "待机",
    running: "进行中",
    blocked: "已暂停",
    waiting: "等待中",
    unknown: "状态暂不可用",
  },
  stepComplete: "完成",
  compactStepCount: {
    memories_one: "{{done}} / {{count}} 条记忆",
    memories_other: "{{done}} / {{count}} 条记忆",
    entities_one: "{{done}} / {{count}} 个实体",
    entities_other: "{{done}} / {{count}} 个实体",
    pages_one: "{{done}} / {{count}} 篇笔记",
    pages_other: "{{done}} / {{count}} 篇笔记",
  },
  moreDetails: "详情",
  processingDetails: "模型与隐私",
  stepCount: {
    memories_one: "{{count}} 条记忆中已完成 {{done}} 条",
    memories_other: "{{count}} 条记忆中已完成 {{done}} 条",
    entities_one: "{{count}} 个实体中已完成 {{done}} 个",
    entities_other: "{{count}} 个实体中已完成 {{done}} 个",
    pages_one: "{{count}} 篇笔记中已完成 {{done}} 篇",
    pages_other: "{{count}} 篇笔记中已完成 {{done}} 篇",
  },
  lane: {
    on_device: "在本机",
    external: "在你的本地服务器",
    anthropic: "在 Anthropic",
    basic: "内置，无需模型",
    none: "未使用模型",
  },
  route: "{{job}}：{{model}}，{{lane}}。",
  routeNoModel: "{{job}}：{{lane}}。",
  jobTitle: {
    everyday: "日常工作",
    synthesis: "写笔记",
  },
  job: {
    everyday: "日常工作",
    synthesis: "写笔记",
  },
  openSettings: "打开设置",
  trustLocal: "在本机运行，数据不会离开你的设备。",
  trustCloud: "{{jobs}}在 {{vendor}} 上运行，该步骤的文本会离开你的设备。其余全部留在本机。",
  suggestions: {
    title: "建议",
    ready_one: "{{count}} 条等待你审阅",
    ready_other: "{{count}} 条等待你审阅",
    notReady_one: "{{count}} 条尚未进入审阅",
    notReady_other: "{{count}} 条尚未进入审阅",
  },
  importHandoff_one:
    "已存入 {{count}} 条记忆，现在即可搜索。文澜会在后台继续沉淀。可在「动态」查看后续进度。",
  importHandoff_other:
    "已存入 {{count}} 条记忆，现在即可搜索。文澜会在后台继续沉淀。可在「动态」查看后续进度。",
  importHandoffOnDemand_one:
    "已存入 {{count}} 条记忆，现在即可搜索。你的 AI 工具用文澜时会顺手沉淀。可在「动态」查看进度。",
  importHandoffOnDemand_other:
    "已存入 {{count}} 条记忆，现在即可搜索。你的 AI 工具用文澜时会顺手沉淀。可在「动态」查看进度。",
  layoutSetting: "动态摘要的位置",
  layoutSettingHelp: "「当前」摘要在动态页面中的位置。仅保存在本设备。",
  layout: {
    rail: "右侧栏",
    card: "信息流上方的卡片",
    timeline: "嵌入信息流",
  },
};

export const hantActivityStatus = {
  title: "活動",
  state: {
    up_to_date: "已就緒",
    off: "隨叫",
    organizing: "沉澱中",
    // Not 暫停: that word is Blocked in this locale.
    waiting_for_idle: "等電腦閒置",
    blocked: "已暫停",
    failed: "失敗",
  },
  statusLabel: "背景工作",
  buttonLabel: "活動，{{state}}",
  headline: {
    up_to_date: "你交給文瀾的內容都已沉澱完畢。",
    off: "沉澱隨叫進行，你的 AI 工具用文瀾時會順手整理。",
    organizing: "文瀾正在沉澱你交給它的內容。",
    waiting_for_idle: "文瀾會在電腦閒置時繼續沉澱。你使用電腦，或電腦忙碌、記憶體不足、溫度過高時，它會先等一等。",
    blocked: "部分沉澱工作在等你處理。",
  },
  asset: {
    memories: "記憶",
    entities: "實體",
    pages: "筆記",
  },
  summary: {
    pagesCurrent: "所有筆記均已更新。",
    memoriesWaiting: "{{count}} 筆待摘要，可直接搜尋",
    entitiesWaiting_one: "{{count}} 筆記憶待辨識主題",
    entitiesWaiting_other: "{{count}} 筆記憶待辨識主題",
  },
  rowAsset: { memories: "記憶", entities: "實體", pages: "筆記" },
  assetOnDemand: {
    memories_one: "已存 {{count}} 則，都可以搜尋",
    memories_other: "已存 {{count}} 則，都可以搜尋",
    entities_one: "{{count}} 則記憶還沒辨識實體",
    entities_other: "{{count}} 則記憶還沒辨識實體",
    pages_one: "{{count}} 篇筆記還沒整理",
    pages_other: "{{count}} 篇筆記還沒整理",
  },
  onDemandFinish: "你的 AI 工具用到時會順手做；想一次做完，輸入 /wenlan steep。",
  assetDone: {
    memories_one: "已存 {{count}} 則，全部已產生摘要並建立關聯",
    memories_other: "已存 {{count}} 則，全部已產生摘要並建立關聯",
    entities: "發現 {{count}} 個，其中 {{confirmed}} 個已收入 Wiki",
    pages_one: "已寫 {{count}} 篇筆記，全部為最新",
    pages_other: "已寫 {{count}} 篇筆記，全部為最新",
  },
  assetEmpty: {
    memories: "還沒有存入內容",
    entities: "還沒有發現實體",
    pages: "還沒有筆記",
  },
  assetRunning: {
    memories: "{{total}} 則中已完成 {{done}} 則的摘要與關聯",
    entities_one: "{{total}} 則記憶中已有 {{done}} 則完成實體掃描",
    entities_other: "{{total}} 則記憶中已有 {{done}} 則完成實體掃描",
    pages: "{{total}} 篇筆記中 {{done}} 篇為最新，其餘正在更新",
  },
  assetBlockedNoModel: {
    memories: "{{count}} 則尚未產生摘要。全部已可搜尋。",
    entities_one: "{{count}} 則記憶尚未掃描實體",
    entities_other: "{{count}} 則記憶尚未掃描實體",
    pages_one: "{{count}} 篇筆記等待更新",
    pages_other: "{{count}} 篇筆記等待更新",
  },
  blockedCause: {
    everyday: "沉澱已暫停，因為尚未設定模型。你的 AI 工具仍可整理記憶。",
    synthesis: "尚未設定撰寫模型，所以筆記會在你的 AI 工具撰寫時更新。",
  },
  blockedCauseUnavailable: {
    everyday: "沉澱已暫停，因為模型暫時無法使用。",
    synthesis: "筆記更新已暫停，因為撰寫模型暫時無法使用。",
  },
  turnOnModel: "啟用模型",
  assetBlockedFailed: {
    memories_one: "{{total}} 則中有 {{count}} 則暫停：產生摘要失敗。全部 {{total}} 則仍可搜尋。",
    memories_other: "{{total}} 則中有 {{count}} 則暫停：產生摘要失敗。全部 {{total}} 則仍可搜尋。",
    entities: "{{total}} 則記憶中有 {{count}} 則暫停：實體辨識失敗。你的記憶不受影響。",
    pages: "{{total}} 篇筆記中有 {{count}} 篇暫停：寫筆記失敗。你的來源不受影響。",
  },
  openActivity: "查看全部活動",
  jobSeparator: "和",
  lastActivity: "最近活動於{{time}}",
  neverActive: "尚未執行過",
  nowTitle: "沉澱",
  nowLoading: "正在查看目前工作…",
  nowError: "無法讀取目前工作。",
  nowIdle: "目前沒有進行中的工作。",
  nowOff: "沉澱隨叫進行。",
  nowRunning: "沉澱正在進行。",
  nowBlocked: "沉澱受阻。",
  nowWaiting: "等待電腦閒置時繼續。",
  nowPaused: "沉澱已暫停。",
  nowUnknown: "目前工作狀態暫時無法讀取。",
  eventsLoading: "正在載入最近活動…",
  eventsError: "無法載入最近活動。",
  readAgain: "重新讀取",
  runningStep: {
    store: "正在儲存記憶。",
    summarize: "正在整理記憶摘要。",
    link: "正在連結相關內容。",
    detect: "正在辨識記憶中的主題。",
    confirm: "正在確認主題。",
    write: "正在更新筆記。",
  },
  failedStep: {
    store_one: "有 {{count}} 則記憶未能存入。",
    store_other: "有 {{count}} 則記憶未能存入。",
    summarize_one: "有 {{count}} 則記憶未能產生摘要。",
    summarize_other: "有 {{count}} 則記憶未能產生摘要。",
    link_one: "有 {{count}} 則記憶未能連結相關內容。",
    link_other: "有 {{count}} 則記憶未能連結相關內容。",
    detect_one: "有 {{count}} 則記憶未能完成主題辨識。",
    detect_other: "有 {{count}} 則記憶未能完成主題辨識。",
    confirm_one: "有 {{count}} 個實體未能確認。",
    confirm_other: "有 {{count}} 個實體未能確認。",
    write_one: "有 {{count}} 篇筆記未能更新。",
    write_other: "有 {{count}} 篇筆記未能更新。",
  },
  failedUnknown: "部分背景工作失敗。",
  showSteps: "展開步驟",
  hideSteps: "收合步驟",
  step: {
    store: "存入",
    summarize: "摘要",
    link: "關聯",
    detect: "辨識",
    confirm: "確認",
    write: "撰寫",
  },
  stepValue: {
    store: "存入後立即可搜尋",
    summarize: "產生摘要與標籤，讓回想更準確",
    link: "與相關記憶和實體建立關聯",
    detect: "你提到的人、專案和工具",
    confirm: "內容足夠充實，可收入 Wiki",
    write: "依據你的來源撰寫並更新筆記",
  },
  stepState: {
    idle: "待機",
    running: "進行中",
    blocked: "已暫停",
    waiting: "等待中",
    unknown: "狀態暫時無法取得",
  },
  stepComplete: "完成",
  compactStepCount: {
    memories_one: "{{done}} / {{count}} 則記憶",
    memories_other: "{{done}} / {{count}} 則記憶",
    entities_one: "{{done}} / {{count}} 個實體",
    entities_other: "{{done}} / {{count}} 個實體",
    pages_one: "{{done}} / {{count}} 篇筆記",
    pages_other: "{{done}} / {{count}} 篇筆記",
  },
  moreDetails: "詳細資料",
  processingDetails: "模型與隱私",
  stepCount: {
    memories_one: "{{count}} 則記憶中已完成 {{done}} 則",
    memories_other: "{{count}} 則記憶中已完成 {{done}} 則",
    entities_one: "{{count}} 個實體中已完成 {{done}} 個",
    entities_other: "{{count}} 個實體中已完成 {{done}} 個",
    pages_one: "{{count}} 篇筆記中已完成 {{done}} 篇",
    pages_other: "{{count}} 篇筆記中已完成 {{done}} 篇",
  },
  lane: {
    on_device: "在本機",
    external: "在你的本機伺服器",
    anthropic: "在 Anthropic",
    basic: "內建，不需模型",
    none: "未使用模型",
  },
  route: "{{job}}：{{model}}，{{lane}}。",
  routeNoModel: "{{job}}：{{lane}}。",
  jobTitle: {
    everyday: "日常工作",
    synthesis: "寫筆記",
  },
  job: {
    everyday: "日常工作",
    synthesis: "寫筆記",
  },
  openSettings: "開啟設定",
  trustLocal: "在本機執行，資料不會離開你的裝置。",
  trustCloud: "{{jobs}}在 {{vendor}} 上執行，該步驟的文字會離開你的裝置。其餘全部留在本機。",
  suggestions: {
    title: "建議",
    ready_one: "{{count}} 則等待你審閱",
    ready_other: "{{count}} 則等待你審閱",
    notReady_one: "{{count}} 則尚未進入審閱",
    notReady_other: "{{count}} 則尚未進入審閱",
  },
  importHandoff_one:
    "已存入 {{count}} 則記憶，現在即可搜尋。文瀾會在背景繼續沉澱。可在「活動」查看後續進度。",
  importHandoff_other:
    "已存入 {{count}} 則記憶，現在即可搜尋。文瀾會在背景繼續沉澱。可在「活動」查看後續進度。",
  importHandoffOnDemand_one:
    "已存入 {{count}} 則記憶，現在即可搜尋。你的 AI 工具用文瀾時會順手沉澱。可在「活動」查看進度。",
  importHandoffOnDemand_other:
    "已存入 {{count}} 則記憶，現在即可搜尋。你的 AI 工具用文瀾時會順手沉澱。可在「活動」查看進度。",
  layoutSetting: "動態摘要的位置",
  layoutSettingHelp: "「目前」摘要在動態頁面中的位置。僅儲存在本裝置。",
  layout: {
    rail: "右側欄",
    card: "資訊流上方的卡片",
    timeline: "嵌入資訊流",
  },
};
