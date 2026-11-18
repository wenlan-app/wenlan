// SPDX-License-Identifier: AGPL-3.0-only
// Fixture library for `?mode=sections`: the Docs, Skills and Rules list pages
// and a rules page, with the names of the sidebar sketch. Fixture data only;
// nothing here says what a daemon does.
import type { KnowledgeFolder, Page, SkillLine, ToolRulesDelivery } from "../src/lib/tauri";
import { PAGES } from "./fixtures";

type Copy = {
  readonly folders: readonly [string, string, string];
  readonly skills: string;
  readonly rules: string;
  readonly docs: readonly [string, string, string];
  readonly skillPages: readonly string[];
  readonly handRules: { readonly title: string; readonly heading: string; readonly lines: readonly string[] };
  readonly managed: readonly { readonly text: string; readonly source: string; readonly sourceId: string }[];
  readonly ownLine: string;
  readonly pending: { readonly text: string; readonly source: string };
  readonly filler: string;
};

const COPY: Record<"en" | "zh", Copy> = {
  en: {
    folders: ["Work", "Travel", "Reading notes"],
    skills: "Skills",
    rules: "Rules",
    docs: ["Pricing and plans", "Release checklist", "Onboarding guide"],
    skillPages: ["Write release notes", "Review a pull request", "Reply to a customer", "Plan a trip", "Summarize a meeting", "Draft a weekly review"],
    handRules: { title: "How we work", heading: "Before a release", lines: ["Ask before changing the plan; when stuck, give options.", "Run the release checklist before tagging a version."] },
    managed: [
      { text: "Run the release checklist before tagging a version.", source: "Release checklist", sourceId: "sec-doc-1" },
      { text: "Never quote a price that is not on the pricing page.", source: "Pricing and plans", sourceId: "sec-doc-0" },
    ],
    ownLine: "Ask before changing the plan; when stuck, give options.",
    pending: { text: "Thank the customer first, then give a date.", source: "Reply to a customer" },
    filler: "Page",
  },
  zh: {
    folders: ["工作", "旅行", "讀書筆記"],
    skills: "技能",
    rules: "規則",
    docs: ["價格與方案", "發版檢查清單", "新人上手指南"],
    skillPages: ["寫發版說明", "審查 Pull Request", "回覆客戶", "規劃旅行", "整理會議記錄", "寫每週回顧"],
    handRules: { title: "我們怎麼做事", heading: "發版之前", lines: ["改計畫前先問；卡住時，給選項。", "打版本標籤前，先跑完發版檢查清單。"] },
    managed: [
      { text: "打版本標籤前，先跑完發版檢查清單。", source: "發版檢查清單", sourceId: "sec-doc-1" },
      { text: "不報價格頁上沒有的價格。", source: "價格與方案", sourceId: "sec-doc-0" },
    ],
    ownLine: "改計畫前先問；卡住時，給選項。",
    pending: { text: "回客戶信先道謝，再給日期。", source: "回覆客戶" },
    filler: "頁面",
  },
};

const DAY = 86_400_000;
const template = PAGES["page-how-i-work"];

function page(id: string, title: string, storagePath: string, ageDays: number, content = `# ${title}\n`, space: string | null = null): Page {
  const at = new Date(Date.now() - ageDays * DAY - 3_600_000).toISOString();
  return { ...template, id, title, storage_path: storagePath, space, domain: space, content, version: 1, created_at: at, last_compiled: at, last_modified: at, user_edited: true };
}

export function sectionsLibrary(lang: string | null): { pages: Page[]; folders: KnowledgeFolder[]; pending: SkillLine; pinned: { id: string; title: string }[]; recent: string[] } {
  const copy = COPY[lang?.startsWith("zh") ? "zh" : "en"];
  const pages: Page[] = [];
  copy.docs.forEach((title, index) => pages.push(page(`sec-doc-${index}`, title, `${title}.md`, [0, 1, 3][index])));
  const sizes = [12, 5, 4];
  copy.folders.forEach((folder, folderIndex) => {
    for (let index = 0; index < sizes[folderIndex]; index++) {
      pages.push(page(`sec-f${folderIndex}-${index}`, `${folder} ${copy.filler} ${index + 1}`, `${folder}/${folder}-${index + 1}.md`, 2 + index + folderIndex));
    }
  });
  copy.skillPages.forEach((title, index) => pages.push(page(`sec-skill-${index}`, title, `${copy.skills}/${title}.md`, index * 2)));
  pages.push(page(
    "sec-rule-hand",
    copy.handRules.title,
    `${copy.rules}/${copy.handRules.title}.md`,
    0,
    `# ${copy.handRules.title}\n\n## ${copy.handRules.heading}\n\n${copy.handRules.lines.map((line) => `- ${line}`).join("\n")}\n`,
  ));
  const managed = copy.managed.map((line, index) =>
    `- [rule] ${line.text} — Source: ${line.source} (${line.sourceId}) r3 at 1794211200 — Quote: ${JSON.stringify(line.text)} <!-- wenlan-rule:sec-rule-${index} -->`);
  pages.push({
    ...page("sec-rule-how", "How I work — Wenlan", "How I work — Wenlan.md", 1, "", "Wenlan"),
    content: `# How I work — Wenlan\n\nThese are this Space's approved working rules.\n\n${managed[0]}\n- ${copy.ownLine}\n${managed[1]}\n`,
  });
  const folders: KnowledgeFolder[] = [...copy.folders, copy.skills, copy.rules].map((name) => ({ path: name, parent_path: "", name }));
  const pending: SkillLine = {
    id: "sec-pending-1",
    kind: "rule",
    text: copy.pending.text,
    source_page_id: "sec-skill-2",
    source_page_title: copy.pending.source,
    source_revision: 2,
    source_revision_at: 1794211200,
    source_quote: copy.pending.text,
  };
  return {
    pages,
    folders,
    pending,
    pinned: [{ id: "sec-doc-1", title: copy.docs[1] }, { id: "sec-skill-5", title: copy.skillPages[5] }],
    recent: ["sec-doc-0", "sec-skill-0", "sec-rule-hand", "sec-doc-2"],
  };
}

export const SECTIONS_DELIVERY: ToolRulesDelivery[] = [
  { id: "claude-code", display_name: "Claude Code", connected: true, method: "file", state: "delivered", delivered_at: Math.floor(Date.now() / 1000) - 3600, rule_count: 3 },
  { id: "codex", display_name: "Codex", connected: true, method: "file", state: "delivered", delivered_at: Math.floor(Date.now() / 1000) - 3600, rule_count: 3 },
  { id: "cursor", display_name: "Cursor", connected: true, method: "mcp", state: "on_request", rule_count: 3 },
  { id: "windsurf", display_name: "Windsurf", connected: true, method: "file", state: "over_limit", rule_count: 3 },
  { id: "zed", display_name: "Zed", connected: false, method: "file", state: "not_connected", rule_count: 3 },
];

/** Answers the commands the section pages read; `undefined` falls through to
 *  the harness's other fixtures. `demo=empty` is a library with pages but no
 *  skills and no rules. */
export function sectionsInvoke(cmd: string, args: Record<string, unknown> | undefined, params: URLSearchParams): unknown {
  const library = sectionsLibrary(params.get("lang"));
  const empty = params.get("demo") === "empty";
  const pages = empty ? library.pages.filter((item) => item.id.startsWith("sec-doc") || item.id.startsWith("sec-f")) : library.pages;
  switch (cmd) {
    case "list_pages":
    case "list_pages_explicit_browse":
      return args?.status === "active" && !args?.offset ? pages : [];
    case "knowledge_folders_list":
      return { folders: empty ? library.folders.slice(0, 3) : library.folders, truncated: false };
    case "get_space_folders":
      return false;
    case "get_page":
    case "get_page_explicit_browse":
      return pages.find((item) => item.id === args?.id) ?? undefined;
    case "get_space_rules": {
      // What the daemon lists: who wrote each rule and which are used in
      // every Space. The page's own text says neither.
      if (empty) return { space_id: "", space_name: "", rules_page_id: null, lines: [] };
      const copy = COPY[params.get("lang")?.startsWith("zh") ? "zh" : "en"];
      const line = (id: string, text: string, source: string, title: string, more: Partial<SkillLine>): SkillLine => ({ id, kind: "rule", text, source_page_id: source, source_page_title: title, source_revision: 3, source_revision_at: 1794211200, source_quote: text, ...more });
      const own = copy.handRules.lines.map((text, index) => line(`sec-hand-${index}`, text, "sec-rule-hand", copy.handRules.title, { hand_written: true, general: true }));
      if (!args?.space) return { space_id: "", space_name: "", rules_page_id: null, lines: own };
      return {
        space_id: "project-wenlan",
        space_name: "Wenlan",
        rules_page_id: "sec-rule-how",
        lines: [
          ...copy.managed.map((item, index) => line(`sec-rule-${index}`, item.text, item.sourceId, item.source, { general: index === 0 })),
          line("sec-own-0", copy.ownLine, "sec-rule-how", "How I work — Wenlan", { hand_written: true }),
          ...own,
        ],
      };
    }
    case "get_rules_delivery":
      return { tools: empty ? [] : SECTIONS_DELIVERY };
    case "get_skills_from_library_enabled":
      return true;
    case "list_space_rules_suggestions":
      return empty ? [] : [{ space_id: "project-wenlan", space_name: "Wenlan", lines: [library.pending] }];
    case "list_skill_suggestions":
    case "list_skill_borrow_records":
    case "list_spaces":
    case "list_onboarding_milestones":
    case "list_indexed_files":
      return [];
    case "list_shares":
      return { shares: [] };
    case "get_activity":
      return { state: "idle", last_activity_at: null, assets: [], everyday: { state: "ready" }, synthesis: { state: "ready" } };
    case "get_profile":
    case "get_space":
    case "get_truth_status":
      return null;
    default:
      return undefined;
  }
}
