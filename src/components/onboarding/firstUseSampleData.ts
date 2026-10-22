// SPDX-License-Identifier: AGPL-3.0-only
//
// Deliberate built-in tutorial data for the FirstUseGuide sample walkthrough.
// This is a product sample feature, not fixture leakage: it is never imported
// from the preview harness, never persisted, never sent to the daemon, and the
// sample-only path issues no read/write daemon calls (clipboard is the only
// exception, and it stays on-device). The tutorial prose is localized per app
// locale via getSampleData; UI chrome around it uses translated copy.
//
// Invariant (enforced by firstUseSampleData.test.ts): every citation quote is
// an exact substring of its source's body, in every locale.

export type SampleSourceType = "memory" | "file";

export interface SampleSource {
  id: string;
  type: SampleSourceType;
  /** Short kind tag shown beside the title (part of the tutorial data). */
  kind: string;
  /** When the source was captured, shown beside the kind tag. */
  when: string;
  title: string;
  location: string;
  excerpt: string;
  body: string[];
  /** File sources carry the on-disk origin of the quoted line. */
  path?: string;
  /** 1-based line number of the quoted line inside `body`. */
  quoteLine?: number;
}

export interface SampleCitation {
  id: string;
  sourceId: string;
  quote: string;
}

export interface SamplePageParagraph {
  /** Optional section label rendered above this paragraph. */
  heading?: string;
  text: string;
  citationIds: string[];
}

export interface SamplePage {
  title: string;
  subtitle: string;
  paragraphs: SamplePageParagraph[];
}

/** The closing beat: a new chat where the AI answers from the page. */
export interface SampleAiExchange {
  question: string;
  answer: string;
}

export type SampleClient = "chatgpt" | "codex" | "claude";

export interface SampleCommands {
  /** Recall works on the user's OWN imported data — never on this sample,
   *  which lives only in the walkthrough and no AI can retrieve. */
  recall: string;
  /** Null when the client connector cannot write back (the ChatGPT web
   *  connector is read-only). The UI shows an explanatory note instead of
   *  a write command it cannot honor. */
  handoff: string | null;
  brief: string;
}

export interface SampleDataset {
  sources: SampleSource[];
  citations: SampleCitation[];
  page: SamplePage;
  ai: SampleAiExchange;
  commands: Record<SampleClient, SampleCommands>;
}

export type SampleLocale = "en" | "zh-Hans" | "zh-Hant";

/** Map an i18next language tag onto the closest tutorial dataset. */
export function resolveSampleLocale(language: string | undefined): SampleLocale {
  const tag = (language ?? "").toLowerCase().replace("_", "-");
  if (tag === "zh-hant" || tag.startsWith("zh-hant-") || tag === "zh-tw" || tag === "zh-hk" || tag === "zh-mo") {
    return "zh-Hant";
  }
  if (tag === "zh" || tag.startsWith("zh-")) {
    return "zh-Hans";
  }
  return "en";
}

const enData: SampleDataset = {
  sources: [
    {
      id: "sample-memory",
      type: "memory",
      kind: "ChatGPT",
      when: "last week",
      title: "Launch date and price",
      location: "ChatGPT chat · v2 launch planning",
      excerpt: "We decided to launch v2 on October 24.",
      body: [
        "We want v2 out before the end of October.",
        "We decided to launch v2 on October 24. We'll price it at $8 a month.",
        "Once the date is final, I'll draft the announcement.",
      ],
    },
    {
      id: "sample-session",
      type: "memory",
      kind: "Claude Code",
      when: "2 days ago",
      title: "Release check",
      location: "Claude Code session · v2 release check",
      excerpt: "The payment review hasn't passed yet, so the launch moves to October 31.",
      body: [
        "Release check: the payment provider is still reviewing our account.",
        "The payment review hasn't passed yet, so the launch moves to October 31.",
        "Everything else is ready to ship.",
      ],
    },
    {
      id: "sample-file",
      type: "file",
      kind: "File",
      when: "yesterday",
      title: "v2-meeting-notes.md",
      location: "product folder / v2-meeting-notes.md",
      excerpt: "Price goes up to $10 a month.",
      body: [
        "Review of the v2 launch plan.",
        "Price goes up to $10 a month. Yearly plan gets 20% off. Launch stays on October 31.",
        "Next: update the pricing page and tell beta users.",
      ],
      path: "product folder / v2-meeting-notes.md",
      quoteLine: 2,
    },
  ],
  citations: [
    {
      id: "sample-cite-1",
      sourceId: "sample-memory",
      quote: "We decided to launch v2 on October 24.",
    },
    {
      id: "sample-cite-2",
      sourceId: "sample-file",
      quote: "Launch stays on October 31.",
    },
    {
      id: "sample-cite-3",
      sourceId: "sample-file",
      quote: "Price goes up to $10 a month.",
    },
    {
      id: "sample-cite-4",
      sourceId: "sample-file",
      quote: "Yearly plan gets 20% off.",
    },
    {
      id: "sample-cite-5",
      sourceId: "sample-session",
      quote: "The payment review hasn't passed yet, so the launch moves to October 31.",
    },
    {
      id: "sample-cite-6",
      sourceId: "sample-memory",
      quote: "We'll price it at $8 a month.",
    },
    {
      id: "sample-cite-7",
      sourceId: "sample-file",
      quote: "Price goes up to $10 a month.",
    },
  ],
  page: {
    title: "v2 launch plan",
    subtitle: "From two AI sessions and meeting notes to one page.",
    paragraphs: [
      {
        heading: "Current plan",
        text: "Launch on October 31.",
        citationIds: ["sample-cite-2"],
      },
      {
        text: "Priced at $10 a month.",
        citationIds: ["sample-cite-3"],
      },
      {
        text: "The yearly plan gets 20% off.",
        citationIds: ["sample-cite-4"],
      },
      {
        heading: "What changed",
        text: "Launch moved from Oct 24 to Oct 31 for the payment review.",
        citationIds: ["sample-cite-1", "sample-cite-5"],
      },
      {
        text: "Price went from $8 to $10 a month.",
        citationIds: ["sample-cite-6", "sample-cite-7"],
      },
    ],
  },
  ai: {
    question: "When does v2 launch, and what does it cost?",
    answer: "From your page “v2 launch plan”: October 31, at $10 a month with 20% off yearly (v2-meeting-notes.md). The date moved from October 24 for the payment review (Claude Code), and the price was $8 before (ChatGPT).",
  },
  commands: {
    chatgpt: {
      recall: "@wenlan recall the decisions and next steps from my imported notes, and list the sources.",
      handoff: null,
      brief: "@wenlan this is a new conversation. Catch me up on earlier work and open todos.",
    },
    codex: {
      recall: "/recall decisions and next steps from my imports",
      handoff: "/handoff",
      brief: "/brief",
    },
    claude: {
      recall: "/recall decisions and next steps from my imports",
      handoff: "/handoff",
      brief: "/brief",
    },
  },
};

const hansData: SampleDataset = {
  sources: [
    {
      id: "sample-memory",
      type: "memory",
      kind: "ChatGPT",
      when: "上周",
      title: "上线日期和价格",
      location: "ChatGPT 对话 · v2 上线讨论",
      excerpt: "我们决定 10 月 24 日上线 v2。",
      body: [
        "我们想在 10 月底前发布 v2。",
        "我们决定 10 月 24 日上线 v2。定价每月 8 美元。",
        "日期确定后，我再来写发布公告。",
      ],
    },
    {
      id: "sample-session",
      type: "memory",
      kind: "Claude Code",
      when: "前天",
      title: "发布前检查",
      location: "Claude Code 会话 · v2 发布前检查",
      excerpt: "支付审核还没通过，所以上线改到 10 月 31 日。",
      body: [
        "发布前检查：支付服务商还在审核我们的账号。",
        "支付审核还没通过，所以上线改到 10 月 31 日。",
        "其余部分都已就绪。",
      ],
    },
    {
      id: "sample-file",
      type: "file",
      kind: "文件",
      when: "昨天",
      title: "v2 会议记录.md",
      location: "产品文件夹 / v2 会议记录.md",
      excerpt: "价格涨到每月 10 美元。",
      body: [
        "v2 上线计划的评审。",
        "价格涨到每月 10 美元。年付优惠 20%。上线维持 10 月 31 日。",
        "接下来：更新价格页，并通知内测用户。",
      ],
      path: "产品文件夹 / v2 会议记录.md",
      quoteLine: 2,
    },
  ],
  citations: [
    {
      id: "sample-cite-1",
      sourceId: "sample-memory",
      quote: "我们决定 10 月 24 日上线 v2。",
    },
    {
      id: "sample-cite-2",
      sourceId: "sample-file",
      quote: "上线维持 10 月 31 日。",
    },
    {
      id: "sample-cite-3",
      sourceId: "sample-file",
      quote: "价格涨到每月 10 美元。",
    },
    {
      id: "sample-cite-4",
      sourceId: "sample-file",
      quote: "年付优惠 20%。",
    },
    {
      id: "sample-cite-5",
      sourceId: "sample-session",
      quote: "支付审核还没通过，所以上线改到 10 月 31 日。",
    },
    {
      id: "sample-cite-6",
      sourceId: "sample-memory",
      quote: "定价每月 8 美元。",
    },
    {
      id: "sample-cite-7",
      sourceId: "sample-file",
      quote: "价格涨到每月 10 美元。",
    },
  ],
  page: {
    title: "v2 上线计划",
    subtitle: "从两段 AI 对话和一份会议记录，整理成一篇笔记。",
    paragraphs: [
      {
        heading: "当前方案",
        text: "10 月 31 日上线。",
        citationIds: ["sample-cite-2"],
      },
      {
        text: "定价每月 10 美元。",
        citationIds: ["sample-cite-3"],
      },
      {
        text: "年付优惠 20%。",
        citationIds: ["sample-cite-4"],
      },
      {
        heading: "改了什么",
        text: "上线从 10 月 24 日改到 10 月 31 日，因为支付审核。",
        citationIds: ["sample-cite-1", "sample-cite-5"],
      },
      {
        text: "价格从每月 8 美元涨到 10 美元。",
        citationIds: ["sample-cite-6", "sample-cite-7"],
      },
    ],
  },
  ai: {
    question: "v2 什么时候上线，要多少钱？",
    answer: "根据你的笔记《v2 上线计划》：v2 于 10 月 31 日上线，每月 10 美元，年付优惠 20%（v2 会议记录.md）。两项都有变动：上线日期因支付审核，从 10 月 24 日改期（Claude Code）；价格从每月 8 美元涨上来（ChatGPT）。",
  },
  commands: {
    chatgpt: {
      recall: "@wenlan 找出我导入的资料中已做出的决定与待办，并列出来源。",
      handoff: null,
      brief: "@wenlan 这是新的对话，请带我接上之前的工作与待办。",
    },
    codex: {
      recall: "/recall 我导入资料中的决定与待办",
      handoff: "/handoff",
      brief: "/brief",
    },
    claude: {
      recall: "/recall 我导入资料中的决定与待办",
      handoff: "/handoff",
      brief: "/brief",
    },
  },
};

const hantData: SampleDataset = {
  sources: [
    {
      id: "sample-memory",
      type: "memory",
      kind: "ChatGPT",
      when: "上週",
      title: "上線日期和價格",
      location: "ChatGPT 對話 · v2 上線討論",
      excerpt: "我們決定 10 月 24 日上線 v2。",
      body: [
        "我們想在 10 月底前推出 v2。",
        "我們決定 10 月 24 日上線 v2。定價每月 8 美元。",
        "日期確定後，我再來寫發布公告。",
      ],
    },
    {
      id: "sample-session",
      type: "memory",
      kind: "Claude Code",
      when: "前天",
      title: "發布前檢查",
      location: "Claude Code 工作階段 · v2 發布前檢查",
      excerpt: "支付審核還沒通過，所以上線改到 10 月 31 日。",
      body: [
        "發布前檢查：支付服務商還在審核我們的帳號。",
        "支付審核還沒通過，所以上線改到 10 月 31 日。",
        "其餘部分都已就緒。",
      ],
    },
    {
      id: "sample-file",
      type: "file",
      kind: "檔案",
      when: "昨天",
      title: "v2 會議記錄.md",
      location: "產品資料夾 / v2 會議記錄.md",
      excerpt: "價格漲到每月 10 美元。",
      body: [
        "v2 上線計畫的評審。",
        "價格漲到每月 10 美元。年繳優惠 20%。上線維持 10 月 31 日。",
        "接下來：更新價格頁，並通知內測使用者。",
      ],
      path: "產品資料夾 / v2 會議記錄.md",
      quoteLine: 2,
    },
  ],
  citations: [
    {
      id: "sample-cite-1",
      sourceId: "sample-memory",
      quote: "我們決定 10 月 24 日上線 v2。",
    },
    {
      id: "sample-cite-2",
      sourceId: "sample-file",
      quote: "上線維持 10 月 31 日。",
    },
    {
      id: "sample-cite-3",
      sourceId: "sample-file",
      quote: "價格漲到每月 10 美元。",
    },
    {
      id: "sample-cite-4",
      sourceId: "sample-file",
      quote: "年繳優惠 20%。",
    },
    {
      id: "sample-cite-5",
      sourceId: "sample-session",
      quote: "支付審核還沒通過，所以上線改到 10 月 31 日。",
    },
    {
      id: "sample-cite-6",
      sourceId: "sample-memory",
      quote: "定價每月 8 美元。",
    },
    {
      id: "sample-cite-7",
      sourceId: "sample-file",
      quote: "價格漲到每月 10 美元。",
    },
  ],
  page: {
    title: "v2 上線計畫",
    subtitle: "從兩段 AI 對話和一份會議記錄，整理成一篇筆記。",
    paragraphs: [
      {
        heading: "目前方案",
        text: "10 月 31 日上線。",
        citationIds: ["sample-cite-2"],
      },
      {
        text: "定價每月 10 美元。",
        citationIds: ["sample-cite-3"],
      },
      {
        text: "年繳優惠 20%。",
        citationIds: ["sample-cite-4"],
      },
      {
        heading: "改了什麼",
        text: "上線從 10 月 24 日改到 10 月 31 日，因為支付審核。",
        citationIds: ["sample-cite-1", "sample-cite-5"],
      },
      {
        text: "價格從每月 8 美元漲到 10 美元。",
        citationIds: ["sample-cite-6", "sample-cite-7"],
      },
    ],
  },
  ai: {
    question: "v2 什麼時候上線，要多少錢？",
    answer: "根據你的筆記《v2 上線計畫》：v2 於 10 月 31 日上線，每月 10 美元，年繳優惠 20%（v2 會議記錄.md）。兩項都有變動：上線日期因支付審核，從 10 月 24 日改期（Claude Code）；價格從每月 8 美元漲上來（ChatGPT）。",
  },
  commands: {
    chatgpt: {
      recall: "@wenlan 找出我匯入的資料中已做出的決定與待辦，並列出來源。",
      handoff: null,
      brief: "@wenlan 這是新的對話，請帶我接上之前的工作與待辦。",
    },
    codex: {
      recall: "/recall 我匯入資料中的決定與待辦",
      handoff: "/handoff",
      brief: "/brief",
    },
    claude: {
      recall: "/recall 我匯入資料中的決定與待辦",
      handoff: "/handoff",
      brief: "/brief",
    },
  },
};

const DATASETS: Record<SampleLocale, SampleDataset> = {
  en: enData,
  "zh-Hans": hansData,
  "zh-Hant": hantData,
};

export function getSampleData(locale: SampleLocale): SampleDataset {
  return DATASETS[locale];
}
