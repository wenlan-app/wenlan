// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
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
import { OWN_IDENTITY, resolveAgentActionName, toolFamilyOf } from "../../../lib/agents";
import { useSteepOnDemand } from "../../../lib/steepMode";
import { useLiveAgents } from "../../connect/connectionState";
import { writeClipboard } from "../page/codeBasisShared";
import { PageIcon } from "../assets/PageIcon";
import { useAskAiCommand } from "./AskAiHint";
import "./writePages.css";

/** The offer shows from this much material, the same floor at which the AI
 *  tool's replies start naming `/wenlan steep`. */
export const WRITE_PAGES_MIN_ITEMS = 3;
/** How many pages one press asks the AI tool for, from the newest topics. */
export const PAGES_PER_RUN = 8;
/** After the user closes the home line it comes back only once this much
 *  more material has arrived. */
const SHOW_AGAIN_AFTER = 10;
/** No claim held and nothing new for this long: the run is over. */
const QUIET_DONE_MS = 90_000;
/** No page and no change in what the tool holds for this long: the tool may
 *  be waiting for the user, or it stopped and its claim has not run out. */
const STUCK_MS = 120_000;
/** A page born this long before the button was pressed still counts; the
 *  daemon's clock and the app's can differ a little. */
const CLOCK_SLACK_MS = 60_000;
const POLL_IDLE_MS = 15_000;
const POLL_RUNNING_MS = 4_000;
/** How often a run reads the pages again while it goes on. */
export const PAGES_REREAD_MS = 10_000;
const MAX_LISTED = 8;

const RUN_KEY = "askAi:writePages:run";
/** The tool the user picked for writing pages, when more than one is connected. */
const TOOL_KEY = "askAi:writePages:tool";
const DISMISSED_KEY = "askAi:writePages:dismissedTotal";
/** The last run let its material go without writing a page. */
const STOPPED_KEY = "askAi:writePages:stopped";

/** One press of "Ask AI to write pages", remembered per viewer. */
export interface WritePagesRun {
  readonly startedAt: number;
  /** The tool's display name, for "Claude Code is writing pages". */
  readonly tool: string | null;
  /** How the command reached the tool: typed into it, or copied for the
   *  user to paste. Kept with the run so a remount (a language switch, a new
   *  window) still says the right thing. */
  readonly handedOver?: "opened" | "copied";
  /** What the tool last held, how much material was waiting then, and when
   *  what it held last changed. Kept with the run so a remount does not
   *  restart the quiet clock, and so material let go with none of it gone
   *  and no page written reads as a stopped run. */
  readonly claim?: { readonly held: number; readonly total: number; readonly at: number };
  /** How many sources each page had when the run started, so a page the
   *  run added sources to counts as updated before (or without) a rewrite. */
  readonly sources?: Readonly<Record<string, number>>;
}

export type WritePagesView =
  | { readonly kind: "hidden" }
  | { readonly kind: "ready"; readonly list: NewMaterialList }
  | { readonly kind: "working"; readonly written: Page[]; readonly writing: boolean; readonly stuck: boolean }
  | {
      readonly kind: "done";
      readonly written: Page[];
      /** Pages that were there before the run and changed in it: they took
       *  new sources, or were brought up to date. */
      readonly updated: Page[];
      /** From the start to the run's last page change. */
      readonly minutes: number;
      readonly left: number;
    };

/** A page's birth time in ms. The daemon writes RFC 3339 or SQLite's
 *  `YYYY-MM-DD HH:MM:SS` (UTC); epoch numbers are accepted too. */
export function pageCreatedMs(page: Pick<Page, "created_at">): number | null {
  return timeMs(page.created_at);
}

/** A page's last change in ms, read as `pageCreatedMs` reads its birth. */
export function pageModifiedMs(page: Pick<Page, "last_modified">): number | null {
  return timeMs(page.last_modified);
}

function timeMs(value: string | null | undefined): number | null {
  const raw = value?.trim() ?? "";
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

/** Each page's source count, for `WritePagesRun.sources`. */
export function sourceCounts(pages: readonly Page[]): Record<string, number> {
  return Object.fromEntries(
    pages.filter((page) => page.creation_kind !== "entity").map((page) => [page.id, page.source_memory_ids?.length ?? 0]),
  );
}

/** Pages that were there before the run started and changed during it,
 *  oldest change first: a run adds sources to the page on a subject instead
 *  of writing a second one, and brings out-of-date pages up to date. A page
 *  counts once it has more sources than at the start (`sourcesAtStart`), as
 *  soon as they are added: its rewrite may come later, after the page list
 *  was last read. */
export function pagesUpdatedSince(
  pages: readonly Page[],
  startedAt: number,
  sourcesAtStart?: Readonly<Record<string, number>>,
): Page[] {
  const written = new Set(pagesWrittenSince(pages, startedAt).map((page) => page.id));
  const gained = (page: Page) => {
    const before = sourcesAtStart?.[page.id];
    return before !== undefined && (page.source_memory_ids?.length ?? 0) > before;
  };
  return pages
    .filter((page) => !written.has(page.id) && page.creation_kind !== "entity")
    .map((page) => ({ page, born: pageCreatedMs(page), at: pageModifiedMs(page) }))
    .filter(
      (entry) =>
        gained(entry.page) ||
        (entry.at !== null && entry.at >= startedAt && (entry.born === null || entry.born < startedAt - CLOCK_SLACK_MS)),
    )
    .sort((a, b) => Math.max(a.at ?? startedAt, startedAt) - Math.max(b.at ?? startedAt, startedAt))
    .map((entry) => entry.page);
}

/** The run's last page change in ms: the newest page born or changed in it. */
export function lastPageChangeMs(written: readonly Page[], updated: readonly Page[], startedAt: number): number {
  return Math.max(
    startedAt,
    ...written.map((page) => pageCreatedMs(page) ?? startedAt),
    ...updated.map((page) => pageModifiedMs(page) ?? startedAt),
  );
}

/** The finished run in one line: the pages written, and those updated. */
export function doneLine(view: { readonly written: readonly Page[]; readonly updated: readonly Page[]; readonly minutes: number }, t: TFunction): string {
  const { written, updated, minutes } = view;
  if (updated.length === 0) return t("askAi.writePages.done", { count: written.length, minutes });
  if (written.length === 0) return t("askAi.writePages.doneUpdatedOnly", { count: updated.length, minutes });
  return t("askAi.writePages.doneAndUpdated", { count: written.length, updated: updated.length, minutes });
}

/** What the offer shows. On demand only: in Automatic mode Wenlan writes the
 *  pages itself. */
export function writePagesView({
  onDemand,
  list,
  pages,
  run,
  dismissedTotal,
  now,
  lastChangeAt,
  minItems = WRITE_PAGES_MIN_ITEMS,
}: {
  readonly onDemand: boolean | undefined;
  readonly list: NewMaterialList | undefined;
  readonly pages: readonly Page[];
  readonly run: WritePagesRun | null;
  readonly dismissedTotal: number | null;
  readonly now: number;
  readonly lastChangeAt: number;
  /** The least material worth an offer. The import finish asks from one
   *  item: the user just brought it in and is looking at it. */
  readonly minItems?: number;
}): WritePagesView {
  if (onDemand !== true || !list) return { kind: "hidden" };
  if (run) {
    const written = pagesWrittenSince(pages, run.startedAt);
    const updated = pagesUpdatedSince(pages, run.startedAt, run.sources);
    const writing = (list.held ?? 0) > 0;
    const quiet = now - lastChangeAt;
    if (!writing && written.length + updated.length > 0 && (quiet >= QUIET_DONE_MS || list.total < minItems)) {
      const last = lastPageChangeMs(written, updated, run.startedAt);
      return {
        kind: "done",
        written,
        updated,
        minutes: Math.max(1, Math.round((last - run.startedAt) / 60_000)),
        left: list.total,
      };
    }
    // Holding material is not progress by itself: a tool that was stopped
    // keeps its claim until the claim runs out.
    return { kind: "working", written, writing, stuck: quiet >= STUCK_MS };
  }
  if (list.total < minItems) return { kind: "hidden" };
  if (dismissedTotal !== null && list.total < dismissedTotal + SHOW_AGAIN_AFTER) return { kind: "hidden" };
  return { kind: "ready", list };
}

/** The tool used most recently, the same one `useAskAiCommand` words the
 *  command for. */
export function recentTool(agents: readonly AgentConnection[]): { family: string; name: string } | null {
  return connectedTools(agents)[0] ?? null;
}

/** Every connected AI tool, one per family, most recently used first. */
export function connectedTools(agents: readonly AgentConnection[]): { family: string; name: string }[] {
  const seen = new Set<string>();
  return agents
    .filter((agent) => !OWN_IDENTITY.test(toolFamilyOf(agent)))
    .sort((a, b) => (b.last_seen_at ?? b.created_at) - (a.last_seen_at ?? a.created_at))
    .map((agent) => toolFamilyOf(agent))
    .filter((family) => (seen.has(family) ? false : (seen.add(family), true)))
    .map((family) => ({ family, name: resolveAgentActionName(family, [...agents]) }));
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

/** One shared state for both places the offer can show: a step in the
 *  getting-started checklist while that is open, and one line at the top of
 *  the Wiki home after. The home reads it once and hands it to whichever is
 *  on screen, so a press in one is the run the other follows. */
export interface WritePagesState {
  /** For the home line: stays away once the user closed it. */
  readonly view: WritePagesView;
  /** For the checklist step, which has no close of its own. */
  readonly stepView: WritePagesView;
  /** For the import finish: offered from the first item. */
  readonly cardView: WritePagesView;
  /** How much material has no page yet. */
  readonly waitingTotal: number;
  readonly onDemand: boolean | undefined;
  /** False until the connected tools are known; nothing shows before. */
  readonly loaded: boolean;
  readonly run: WritePagesRun | null;
  /** The tool the button opens or copies the command for: the one the user
   *  picked, else the one used most recently. */
  readonly tool: { family: string; name: string } | null;
  /** Every connected tool, most recently used first. */
  readonly tools: readonly { family: string; name: string }[];
  /** Use this tool for writing pages from now on (remembered per viewer). */
  readonly chooseTool: (family: string) => void;
  readonly toolName: string;
  readonly opensTool: boolean;
  readonly command: string | null;
  readonly copied: boolean;
  /** "12 notes, 6 chats, and 4 files" waiting, already worded. */
  readonly waiting: string;
  readonly start: () => Promise<void>;
  readonly copyCommand: () => Promise<boolean>;
  /** Hides the home line until much more material arrives, and ends the run. */
  readonly close: () => void;
  /** The last run ended without a page: its material came back untouched. */
  readonly stopped?: boolean;
}

export function useWritePages(pages: readonly Page[]): WritePagesState {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const onDemand = useSteepOnDemand();
  const agents = useLiveAgents();
  const [picked, setPicked] = useState<string | null>(() => readJson<string>(TOOL_KEY));
  const tools = agents.isSuccess ? connectedTools(agents.data) : [];
  const tool = tools.find((candidate) => candidate.family === picked) ?? tools[0] ?? null;
  const command = useAskAiCommand("steep", tool?.family);
  const [run, setRunState] = useState<WritePagesRun | null>(() => readJson<WritePagesRun>(RUN_KEY));
  const [dismissedTotal, setDismissedTotal] = useState<number | null>(() => readJson<number>(DISMISSED_KEY));
  const [copied, setCopied] = useState(false);
  const [stopped, setStoppedState] = useState(() => readJson<boolean>(STOPPED_KEY) === true);
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
  // When the tool last did something: the newest page it wrote, by that
  // page's own birth time, or the last time what it holds changed (kept with
  // the run), so reopening the home later does not restart the quiet clock.
  // Only a change counts: a tool killed mid-run holds its material until the
  // claim runs out, and all that time nothing is being written.
  const updated = run ? pagesUpdatedSince(pages, run.startedAt, run.sources) : [];
  const lastChangeAt = Math.max(run?.claim?.at ?? 0, run ? lastPageChangeMs(written, updated, run.startedAt) : 0);

  const material = list.data;
  useEffect(() => {
    if (!run || !material) return;
    const held = material.held ?? 0;
    const claim = run.claim;
    const keep = (next: WritePagesRun | null) => {
      writeJson(RUN_KEY, next);
      setRunState(next);
    };
    if (held > 0) {
      if (claim?.held !== held) keep({ ...run, claim: { held, total: material.total, at: Date.now() } });
      else if (claim.total !== material.total) keep({ ...run, claim: { ...claim, total: material.total } });
      return;
    }
    if (!claim || claim.held === 0) return;
    // The tool let its material go with none of it written up (nothing left
    // the list, no page landed): the run stopped. End it; the offer says so.
    if (written.length === 0 && material.total >= claim.total) {
      keep(null);
      writeJson(STOPPED_KEY, true);
      setStoppedState(true);
      return;
    }
    keep({ ...run, claim: { held: 0, total: material.total, at: Date.now() } });
  }, [run, material, written.length]);
  const lastTotal = useRef(list.data?.total);

  // A page the tool wrote takes its material out of the list, whether or not
  // the run started here. The Wiki's own page list does not poll, so read it
  // again then, and the page shows up.
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

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2_000);
    return () => clearTimeout(timer);
  }, [copied]);

  const inputs = { onDemand, list: list.data, pages, run, now, lastChangeAt };
  const view = writePagesView({ ...inputs, dismissedTotal });
  const stepView = writePagesView({ ...inputs, dismissedTotal: null });
  const cardView = writePagesView({ ...inputs, dismissedTotal: null, minItems: 1 });

  // A rewrite, or sources added to the page on a subject, changes a page
  // without changing how much material waits, so the read above misses it.
  // While a run goes on, read the pages again every so often, and once more
  // when the tool lets go of its material and when the run is done, so the
  // done line counts every page the run changed.
  const runningSince = run?.startedAt;
  const finished = view.kind === "done";
  useEffect(() => {
    if (runningSince === undefined || finished) return;
    const timer = setInterval(() => void queryClient.invalidateQueries({ queryKey: ["pages"] }), PAGES_REREAD_MS);
    return () => clearInterval(timer);
  }, [runningSince, finished, queryClient]);
  const letGo = runningSince !== undefined && list.data !== undefined && (list.data.held ?? 0) === 0;
  useEffect(() => {
    if (letGo || finished) void queryClient.invalidateQueries({ queryKey: ["pages"] });
  }, [letGo, finished, queryClient]);

  const toolName = run?.tool ?? tool?.name ?? t("askAi.writePages.yourTool");
  const opensTool = tool !== null && PROMPT_LINK_FAMILIES.includes(tool.family);

  const kinds = list.data?.kinds ?? {};
  const total = list.data?.total ?? 0;
  const parts = KIND_ORDER
    .filter((kind) => (kinds[kind] ?? 0) > 0)
    .map((kind) => t(`askAi.writePages.kinds.${kind}`, { count: kinds[kind] }));
  const known = KIND_ORDER.reduce((sum, kind) => sum + (kinds[kind] ?? 0), 0);
  const waiting = parts.length === 0 || known < total
    ? t("askAi.writePages.kinds.item", { count: total })
    : listParts(parts, i18n.language);

  const setRun = (next: WritePagesRun | null) => {
    writeJson(RUN_KEY, next);
    setRunState(next);
  };
  const chooseTool = (family: string) => {
    writeJson(TOOL_KEY, family);
    setPicked(family);
  };
  const clearStopped = () => {
    writeJson(STOPPED_KEY, null);
    setStoppedState(false);
  };
  const close = () => {
    clearStopped();
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
    clearStopped();
    const begun: WritePagesRun = { startedAt, tool: tool?.name ?? null, sources: sourceCounts(pages) };
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

  return {
    view,
    stepView,
    cardView,
    waitingTotal: total,
    onDemand,
    loaded: agents.isSuccess,
    run,
    tool,
    tools,
    chooseTool,
    toolName,
    opensTool,
    command,
    copied,
    waiting,
    start,
    copyCommand,
    close,
    stopped,
  };
}

/** What is waiting, worded for where the user is: before any page exists all
 *  of it, after that only what is left. */
export function waitingLine(state: WritePagesState, pages: readonly Page[], t: TFunction): string {
  const hasPages = pages.some((page) => page.creation_kind === "distilled");
  return t(hasPages ? "askAi.writePages.leadMore" : "askAi.writePages.lead", { list: state.waiting });
}

/** What a run is doing. Until the tool holds material or a page lands it
 *  has not started: the command is waiting in the tool for Enter (or on the
 *  clipboard), so saying it is writing would be wrong. */
export function workingHeadline(
  state: WritePagesState,
  view: Extract<WritePagesView, { kind: "working" }>,
  t: TFunction,
  /** How many pages the run asked for, for "3 of 8 ready". */
  of?: number,
): string {
  if (view.written.length > 0) {
    return of !== undefined && view.written.length <= of
      ? t("askAi.writePages.workingCountOf", { tool: state.toolName, count: view.written.length, total: of })
      : t("askAi.writePages.workingCount", { tool: state.toolName, count: view.written.length });
  }
  if (view.writing) return t("askAi.writePages.working", { tool: state.toolName });
  return state.run?.handedOver === "opened"
    ? t("askAi.writePages.pressEnter", { tool: state.toolName })
    : t("askAi.writePages.pasteIt", { tool: state.toolName });
}

/** Pages as one-click rows: the ones this run wrote ticked, the rest with
 *  their page icon, and a spinner row while the tool is still writing. */
export function WrittenPages({
  rows,
  writing,
  onSelectPage,
  fresh,
}: {
  readonly rows: readonly Page[];
  readonly writing: boolean;
  readonly onSelectPage: (pageId: string) => void;
  /** Rows to tick. Without it every row is one the run wrote. */
  readonly fresh?: ReadonlySet<string>;
}) {
  const { t } = useTranslation();
  if (rows.length === 0 && !writing) return null;
  return (
    <ul className="write-pages-list">
      {rows.slice(0, MAX_LISTED).map((page) => (
        <li key={page.id}>
          {fresh === undefined || fresh.has(page.id) ? (
            <CheckCircle aria-hidden="true" weight="fill" className="write-pages-row-icon write-pages-row-icon--done" />
          ) : (
            <PageIcon className="write-pages-row-icon write-pages-row-icon--page" size={14} />
          )}
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
  );
}

/** The one fallback once the command went out: copy it again. Opened in the
 *  tool, it shows only until a page lands ("Didn't open?"); copied, it stays
 *  for as long as the run is waiting. */
export function copyAgainLabel(state: WritePagesState, written: number, t: TFunction): string | null {
  if (state.copied) return t("askAi.copied");
  if (state.run?.handedOver === "copied") return t("askAi.writePages.copyCommand");
  if (state.run?.handedOver === "opened" && written === 0) return t("askAi.writePages.didntOpen");
  return null;
}

interface WritePagesLineProps {
  readonly state: WritePagesState;
  readonly pages: readonly Page[];
  readonly onConnectTool?: () => void;
  /** Opens the steep settings, where the local model is chosen. */
  readonly onUseLocalModel?: () => void;
}

/**
 * One line at the top of the Wiki home, in place of a card: what is waiting
 * and one button that hands it to the user's AI tool, then the same line
 * follows the run. The pages it writes land, ticked, in the home's page list
 * right below, so the line itself lists none. Shown only once the
 * getting-started checklist, which carries the same offer as a step, is out
 * of the way.
 */
export function WritePagesLine({ state, pages, onConnectTool, onUseLocalModel }: WritePagesLineProps) {
  const { t } = useTranslation();
  const { view } = state;
  if (view.kind === "hidden" || !state.loaded) return null;

  const closeButton = (
    <button
      type="button"
      className="write-pages-close"
      aria-label={t("askAi.hide")}
      title={t("askAi.hide")}
      onClick={state.close}
    >
      <X aria-hidden="true" weight="regular" />
    </button>
  );

  if (view.kind === "ready") {
    // A tool the app cannot open gets the command to paste, and says so.
    const pasteHint = state.tool && !state.opensTool
      ? t("askAi.writePages.copiesCommand", { tool: state.tool.name })
      : null;
    return (
      <div className="write-pages-line" data-testid="write-pages-line" data-state="ready" aria-live="polite">
        <div className="write-pages-line-row">
          <Sparkle aria-hidden="true" weight="regular" className="write-pages-icon" />
          <p className="write-pages-text">
            {state.stopped ? <span className="write-pages-stopped">{t("askAi.writePages.stopped")}</span> : null}
            {waitingLine(state, pages, t)}
            {pasteHint ? <span className="write-pages-hint">{pasteHint}</span> : null}
          </p>
          <div className="write-pages-actions">
            {state.tool ? (
              <button
                type="button"
                className="write-pages-go"
                disabled={state.command === null}
                onClick={() => void state.start()}
              >
                {/* Worded like the checklist step, so both places say the same. */}
                {state.copied
                  ? t("askAi.copied")
                  : state.opensTool
                    ? t("askAi.writePages.askIn", { tool: state.tool.name })
                    : t("askAi.writePages.copyCommand")}
              </button>
            ) : (
              <>
                {onConnectTool ? (
                  <button type="button" className="write-pages-go" onClick={onConnectTool}>
                    {t("askAi.writePages.connect")}
                  </button>
                ) : null}
                {onUseLocalModel ? (
                  <button type="button" className="write-pages-link" onClick={onUseLocalModel}>
                    {t("askAi.writePages.useLocalModel")}
                  </button>
                ) : null}
              </>
            )}
          </div>
          {closeButton}
        </div>
      </div>
    );
  }

  if (view.kind === "working") {
    const hint = view.stuck ? t("askAi.writePages.stuck", { tool: state.toolName }) : null;
    const copyAgain = copyAgainLabel(state, view.written.length, t);
    return (
      <div className="write-pages-line" data-testid="write-pages-line" data-state="working" aria-live="polite">
        <div className="write-pages-line-row">
          <CircleNotch aria-hidden="true" weight="bold" className="write-pages-icon write-pages-spin" />
          <p className="write-pages-text">
            {workingHeadline(state, view, t)}
            {hint ? <span className="write-pages-hint">{hint}</span> : null}
          </p>
          <div className="write-pages-actions">
            {copyAgain ? (
              <button
                type="button"
                className="write-pages-link"
                disabled={state.command === null}
                onClick={() => void state.copyCommand()}
              >
                {copyAgain}
              </button>
            ) : null}
          </div>
          {closeButton}
        </div>
      </div>
    );
  }

  return (
    <div className="write-pages-line" data-testid="write-pages-line" data-state="done" aria-live="polite">
      <div className="write-pages-line-row">
        <CheckCircle aria-hidden="true" weight="fill" className="write-pages-icon write-pages-icon--done" />
        <p className="write-pages-text">
          {doneLine(view, t)}
        </p>
        <div className="write-pages-actions">
          {view.left >= WRITE_PAGES_MIN_ITEMS && state.tool ? (
            <button type="button" className="write-pages-link" disabled={state.command === null} onClick={() => void state.start()}>
              {t("askAi.writePages.again")}
            </button>
          ) : null}
        </div>
        {closeButton}
      </div>
    </div>
  );
}
