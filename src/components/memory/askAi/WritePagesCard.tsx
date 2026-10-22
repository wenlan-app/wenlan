// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { CheckCircle, CircleNotch, Sparkle, X } from "@phosphor-icons/react";
import {
  listNewMaterial,
  openToolWithPrompt,
  PROMPT_LINK_FAMILIES,
  type AgentConnection,
  type NewMaterialList,
  type Page,
} from "../../../lib/tauri";
import { OWN_IDENTITY, resolveAgentDisplayName, toolFamilyOf } from "../../../lib/agents";
import { useSteepOnDemand } from "../../../lib/steepMode";
import { useLiveAgents } from "../../connect/connectionState";
import { writeClipboard } from "../page/codeBasisShared";
import { useAskAiCommand } from "./AskAiHint";
import "./writePagesCard.css";

/** The card shows from this much material, the same floor at which the AI
 *  tool's replies start naming `/wenlan steep`. */
export const WRITE_PAGES_MIN_ITEMS = 3;
/** After the user closes the card it comes back only once this much more
 *  material has arrived. */
const SHOW_AGAIN_AFTER = 10;
/** No claim held and nothing new for this long: the run is over. */
const QUIET_DONE_MS = 90_000;
/** Nothing at all for this long: the tool may be waiting for the user. */
const STUCK_MS = 120_000;
/** A page born this long before the button was pressed still counts; the
 *  daemon's clock and the app's can differ a little. */
const CLOCK_SLACK_MS = 60_000;
const POLL_IDLE_MS = 15_000;
const POLL_RUNNING_MS = 4_000;
const MAX_LISTED = 8;

const RUN_KEY = "askAi:writePages:run";
const DISMISSED_KEY = "askAi:writePages:dismissedTotal";

/** One press of "Ask AI to write pages", remembered per viewer. */
export interface WritePagesRun {
  readonly startedAt: number;
  /** The tool's display name, for "Claude Code is writing pages". */
  readonly tool: string | null;
  /** How the command reached the tool: typed into it, or copied for the
   *  user to paste. Kept with the run so a remount (a language switch, a new
   *  window) still says the right thing. */
  readonly handedOver?: "opened" | "copied";
}

export type WritePagesView =
  | { readonly kind: "hidden" }
  | { readonly kind: "ready"; readonly list: NewMaterialList }
  | { readonly kind: "working"; readonly written: Page[]; readonly writing: boolean; readonly stuck: boolean }
  | { readonly kind: "done"; readonly written: Page[]; readonly minutes: number; readonly left: number };

/** A page's birth time in ms. The daemon writes RFC 3339 or SQLite's
 *  `YYYY-MM-DD HH:MM:SS` (UTC); epoch numbers are accepted too. */
export function pageCreatedMs(page: Pick<Page, "created_at">): number | null {
  const raw = page.created_at?.trim() ?? "";
  if (/^\d+$/.test(raw)) {
    const n = Number(raw);
    return n < 1e12 ? n * 1000 : n;
  }
  const iso = raw.replace(" ", "T");
  const ms = Date.parse(/(Z|[+-]\d\d:?\d\d)$/i.test(iso) ? iso : `${iso}Z`);
  return Number.isNaN(ms) ? null : ms;
}

/** Pages written from material since the run started, oldest first. */
export function pagesWrittenSince(pages: readonly Page[], startedAt: number): Page[] {
  return pages
    .filter((page) => page.creation_kind === "distilled")
    .map((page) => ({ page, at: pageCreatedMs(page) }))
    .filter((entry): entry is { page: Page; at: number } => entry.at !== null && entry.at >= startedAt - CLOCK_SLACK_MS)
    .sort((a, b) => a.at - b.at)
    .map((entry) => entry.page);
}

/** What the card shows. On demand only: in Automatic mode Wenlan writes the
 *  pages itself. */
export function writePagesView({
  onDemand,
  list,
  pages,
  run,
  dismissedTotal,
  now,
  lastChangeAt,
}: {
  readonly onDemand: boolean | undefined;
  readonly list: NewMaterialList | undefined;
  readonly pages: readonly Page[];
  readonly run: WritePagesRun | null;
  readonly dismissedTotal: number | null;
  readonly now: number;
  readonly lastChangeAt: number;
}): WritePagesView {
  if (onDemand !== true || !list) return { kind: "hidden" };
  if (run) {
    const written = pagesWrittenSince(pages, run.startedAt);
    const writing = (list.held ?? 0) > 0;
    const quiet = now - lastChangeAt;
    if (!writing && written.length > 0 && (quiet >= QUIET_DONE_MS || list.total < WRITE_PAGES_MIN_ITEMS)) {
      const last = Math.max(...written.map((page) => pageCreatedMs(page) ?? run.startedAt));
      return {
        kind: "done",
        written,
        minutes: Math.max(1, Math.round((last - run.startedAt) / 60_000)),
        left: list.total,
      };
    }
    return { kind: "working", written, writing, stuck: !writing && quiet >= STUCK_MS };
  }
  if (list.total < WRITE_PAGES_MIN_ITEMS) return { kind: "hidden" };
  if (dismissedTotal !== null && list.total < dismissedTotal + SHOW_AGAIN_AFTER) return { kind: "hidden" };
  return { kind: "ready", list };
}

/** The tool used most recently, the same one `useAskAiCommand` words the
 *  command for. */
export function recentTool(agents: readonly AgentConnection[]): { family: string; name: string } | null {
  const tools = agents
    .filter((agent) => !OWN_IDENTITY.test(toolFamilyOf(agent)))
    .sort((a, b) => (b.last_seen_at ?? b.created_at) - (a.last_seen_at ?? a.created_at));
  if (tools.length === 0) return null;
  const family = toolFamilyOf(tools[0]);
  return { family, name: resolveAgentDisplayName(family, [...agents]) };
}

const KIND_ORDER = ["note", "chat", "file", "capture"] as const;

/** "12 notes, 6 chats, and 4 files". Chinese lists take 、 between every
 *  item; ListFormat's 和 there loses the space before a number. */
export function listParts(parts: readonly string[], language: string): string {
  if (language.toLowerCase().startsWith("zh")) return parts.join("、");
  return new Intl.ListFormat(language, { style: "long", type: "conjunction" }).format(parts);
}

function readJson<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? null : (JSON.parse(raw) as T);
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Remembered for this session only.
  }
}

interface WritePagesCardProps {
  readonly pages: readonly Page[];
  readonly onSelectPage: (pageId: string) => void;
  readonly onConnectTool?: () => void;
  /** Opens the steep settings, where the local model is chosen. */
  readonly onUseLocalModel?: () => void;
}

/**
 * After material comes in, the Wiki home offers to have the user's AI tool
 * turn it into pages: one button opens the tool with `/wenlan steep` typed
 * in, then the card follows the pages as they land.
 */
export function WritePagesCard({ pages, onSelectPage, onConnectTool, onUseLocalModel }: WritePagesCardProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const onDemand = useSteepOnDemand();
  const agents = useLiveAgents();
  const command = useAskAiCommand("steep");
  const [run, setRunState] = useState<WritePagesRun | null>(() => readJson<WritePagesRun>(RUN_KEY));
  const [dismissedTotal, setDismissedTotal] = useState<number | null>(() => readJson<number>(DISMISSED_KEY));
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const list = useQuery({
    queryKey: ["newMaterial"],
    queryFn: listNewMaterial,
    enabled: onDemand === true,
    refetchOnWindowFocus: true,
    refetchInterval: run ? POLL_RUNNING_MS : POLL_IDLE_MS,
    refetchIntervalInBackground: false,
  });

  const written = run ? pagesWrittenSince(pages, run.startedAt) : [];
  const signature = `${list.data?.total ?? ""}:${list.data?.held ?? 0}:${written.length}`;
  const [lastChangeAt, setLastChangeAt] = useState(() => {
    const newest = written.map((page) => pageCreatedMs(page) ?? 0);
    return Math.max(run?.startedAt ?? 0, ...newest, 0);
  });
  const lastSignature = useRef(signature);
  const lastTotal = useRef(list.data?.total);
  useEffect(() => {
    if (signature === lastSignature.current) return;
    lastSignature.current = signature;
    setLastChangeAt(Date.now());
  }, [signature]);

  // A page the tool wrote takes its material out of the list, whether or not
  // the run started from this card. The Wiki's own page list does not poll,
  // so read it again then, and the page shows up.
  useEffect(() => {
    const total = list.data?.total;
    if (total !== undefined && lastTotal.current !== undefined && total < lastTotal.current) {
      void queryClient.invalidateQueries({ queryKey: ["pages"] });
    }
    lastTotal.current = total;
  }, [list.data?.total, queryClient]);

  useEffect(() => {
    if (!run) return;
    const timer = setInterval(() => setNow(Date.now()), 5_000);
    return () => clearInterval(timer);
  }, [run]);

  const view = writePagesView({
    onDemand,
    list: list.data,
    pages,
    run,
    dismissedTotal,
    now,
    lastChangeAt,
  });
  if (view.kind === "hidden" || !agents.isSuccess) return null;

  const tool = recentTool(agents.data);
  const toolName = run?.tool ?? tool?.name ?? t("askAi.writePages.yourTool");
  const opensTool = tool !== null && PROMPT_LINK_FAMILIES.includes(tool.family);

  const setRun = (next: WritePagesRun | null) => {
    writeJson(RUN_KEY, next);
    setRunState(next);
  };
  const close = () => {
    const total = list.data?.total ?? 0;
    writeJson(DISMISSED_KEY, total);
    setDismissedTotal(total);
    setRun(null);
  };
  const copyCommand = async () => {
    if (command === null) return false;
    const ok = await writeClipboard(command);
    if (ok) setCopied(true);
    return ok;
  };
  const start = async () => {
    if (command === null) return;
    const startedAt = Date.now();
    setNow(startedAt);
    setLastChangeAt(startedAt);
    const begun: WritePagesRun = { startedAt, tool: tool?.name ?? null };
    setRun(begun);
    if (opensTool && tool) {
      try {
        await openToolWithPrompt(tool.family, command);
        setRun({ ...begun, handedOver: "opened" });
        return;
      } catch {
        // Not installed here; the copied command still works.
      }
    }
    await copyCommand();
    setRun({ ...begun, handedOver: "copied" });
  };

  const closeButton = (
    <button
      type="button"
      className="write-pages-close"
      aria-label={t("askAi.hide")}
      title={t("askAi.hide")}
      onClick={close}
    >
      <X aria-hidden="true" weight="regular" />
    </button>
  );

  const writtenList = (rows: Page[], writing: boolean) => (rows.length > 0 || writing ? (
    <ul className="write-pages-list">
      {rows.slice(0, MAX_LISTED).map((page) => (
        <li key={page.id}>
          <CheckCircle aria-hidden="true" weight="fill" className="write-pages-row-icon write-pages-row-icon--done" />
          <button type="button" className="write-pages-row-title" onClick={() => onSelectPage(page.id)}>
            {page.title}
          </button>
          {page.source_memory_ids.length > 0 ? (
            <span className="write-pages-row-meta">
              {t("askAi.writePages.sources", { count: page.source_memory_ids.length })}
            </span>
          ) : null}
        </li>
      ))}
      {rows.length > MAX_LISTED ? (
        <li className="write-pages-row-more">{t("askAi.writePages.more", { count: rows.length - MAX_LISTED })}</li>
      ) : null}
      {writing ? (
        <li className="write-pages-row-writing">
          <CircleNotch aria-hidden="true" weight="bold" className="write-pages-row-icon write-pages-spin" />
          <span>{t("askAi.writePages.writingNext")}</span>
        </li>
      ) : null}
    </ul>
  ) : null);

  if (view.kind === "ready") {
    const kinds = view.list.kinds ?? {};
    const parts = KIND_ORDER
      .filter((kind) => (kinds[kind] ?? 0) > 0)
      .map((kind) => t(`askAi.writePages.kinds.${kind}`, { count: kinds[kind] }));
    const known = KIND_ORDER.reduce((sum, kind) => sum + (kinds[kind] ?? 0), 0);
    if (parts.length === 0 || known < view.list.total) {
      parts.length = 0;
      parts.push(t("askAi.writePages.kinds.item", { count: view.list.total }));
    }
    const listed = listParts(parts, i18n.language);
    // Once pages exist, "your material is in" no longer fits: only this part
    // of it is waiting.
    const hasPages = pages.some((page) => page.creation_kind === "distilled");
    return (
      <section className="write-pages-card" data-testid="write-pages-card" data-state="ready" aria-live="polite">
        <div className="write-pages-head">
          <Sparkle aria-hidden="true" weight="regular" className="write-pages-icon" />
          <p className="write-pages-lead">{t(hasPages ? "askAi.writePages.leadMore" : "askAi.writePages.lead", { list: listed })}</p>
          {closeButton}
        </div>
        {tool ? (
          <>
            <div className="write-pages-actions">
              <button
                type="button"
                className="write-pages-primary"
                disabled={command === null}
                onClick={() => void start()}
              >
                {t("askAi.writePages.ask")}
              </button>
              <button type="button" className="write-pages-link" disabled={command === null} onClick={() => void copyCommand()}>
                {copied ? t("askAi.copied") : t("askAi.writePages.copyCommand")}
              </button>
            </div>
            <p className="write-pages-sub">
              {opensTool
                ? t("askAi.writePages.opensTool", { tool: tool.name })
                : t("askAi.writePages.copiesCommand", { tool: tool.name })}
            </p>
          </>
        ) : (
          <>
            <p className="write-pages-sub">{t("askAi.writePages.noTool")}</p>
            <div className="write-pages-actions">
              {onConnectTool ? (
                <button type="button" className="write-pages-primary" onClick={onConnectTool}>
                  {t("askAi.writePages.connect")}
                </button>
              ) : null}
              {onUseLocalModel ? (
                <button type="button" className="write-pages-secondary" onClick={onUseLocalModel}>
                  {t("askAi.writePages.useLocalModel")}
                </button>
              ) : null}
            </div>
          </>
        )}
      </section>
    );
  }

  if (view.kind === "working") {
    return (
      <section className="write-pages-card" data-testid="write-pages-card" data-state="working" aria-live="polite">
        <div className="write-pages-head">
          <CircleNotch aria-hidden="true" weight="bold" className="write-pages-icon write-pages-spin" />
          <p className="write-pages-lead">
            {view.written.length > 0
              ? t("askAi.writePages.workingCount", { tool: toolName, count: view.written.length })
              : t("askAi.writePages.working", { tool: toolName })}
          </p>
          {closeButton}
        </div>
        {view.written.length === 0 && !view.writing ? (
          <p className="write-pages-sub">
            {run?.handedOver !== "opened"
              ? t("askAi.writePages.pasteIt", { tool: toolName })
              : t("askAi.writePages.pressEnter", { tool: toolName })}
          </p>
        ) : null}
        {writtenList(view.written, view.writing)}
        {view.stuck ? <p className="write-pages-sub">{t("askAi.writePages.stuck", { tool: toolName })}</p> : null}
      </section>
    );
  }

  return (
    <section className="write-pages-card" data-testid="write-pages-card" data-state="done" aria-live="polite">
      <div className="write-pages-head">
        <CheckCircle aria-hidden="true" weight="fill" className="write-pages-icon write-pages-icon--done" />
        <p className="write-pages-lead">
          {t("askAi.writePages.done", { count: view.written.length, minutes: view.minutes })}
        </p>
        {closeButton}
      </div>
      {writtenList(view.written, false)}
      {view.left > 0 ? <p className="write-pages-sub">{t("askAi.writePages.left", { count: view.left })}</p> : null}
      {view.left >= WRITE_PAGES_MIN_ITEMS && tool ? (
        <div className="write-pages-actions">
          <button type="button" className="write-pages-secondary" disabled={command === null} onClick={() => void start()}>
            {t("askAi.writePages.again")}
          </button>
        </div>
      ) : null}
    </section>
  );
}
