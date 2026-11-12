// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { SteepMoon } from "../things/ThingSteeped";
import { ranOvernight, steepedToday } from "../../../lib/graph/lastNight";
import { getAiMemoryImport, listSetAsideMaterial, listThings, releaseSetAsideMaterial, stopAiMemoryImport, type SetAsideGroup } from "../../../lib/tauri";
import AiMemoryDialog from "../sources/AiMemoryDialog";
import { useLiveAgents } from "../../connect/connectionState";
import { AskInToolButton } from "../askAi/askInTool";
import { useAskAiCommand } from "../askAi/AskAiHint";
import { connectedTools, pickedWritePagesTool } from "../askAi/WritePages";
import { THINGS_QUERY_KEY } from "./WorkingOn";

/** Most names a night line writes out; the rest are "and more". */
const STEEPED_NAMED = 2;

export const SET_ASIDE_QUERY_KEY = ["steep", "setAside"] as const;

/** Names written out, then "and more" when there are others (UX-HOME-2:
 *  names, never a count). */
function namesLine(t: TFunction, names: readonly string[]): string {
  const shown = names.slice(0, STEEPED_NAMED).join(t("pages.overview.steepedSeparator"));
  return names.length > STEEPED_NAMED ? `${shown} ${t("pages.overview.steepedMore")}` : shown;
}

/** The words handed to the user's AI tool: its own steep command, then each
 *  set-aside group as the exact claim to make, so the skill claims these
 *  items and nothing else. */
export function unwrittenPrompt(t: TFunction, command: string, groups: readonly SetAsideGroup[]): string {
  const claims = groups
    .filter((group) => group.items.length > 0)
    .map((group) => {
      const spaces = [...new Set(group.items.map((item) => item.space).filter((space): space is string => !!space))];
      const where = spaces.length === 1 ? ` in Space «${spaces[0]}»` : "";
      return `claim_new item_ids ${JSON.stringify(group.items.map((item) => item.item_id))} titled «${group.name}»${where}`;
    })
    .join("; ");
  return t("pages.overview.unwrittenPrompt", { command, groups: claims, interpolation: { escapeValue: false } });
}

/** Groups Automatic tried to write as a page and set aside. An older daemon
 *  without the route reads as none. */
export function useSetAside() {
  return useQuery({ queryKey: SET_ASIDE_QUERY_KEY, queryFn: listSetAsideMaterial, refetchInterval: 60_000, retry: false });
}

/**
 * "Not written yet": the material Automatic set aside instead of writing a
 * page from it, by name. With an AI tool connected, one button hands that
 * tool its steep command for exactly these groups; with none, "Try again"
 * gives them back to Automatic.
 */
function UnwrittenLine({ groups }: { readonly groups: readonly SetAsideGroup[] }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const agents = useLiveAgents();
  const tools = agents.isSuccess ? connectedTools(agents.data) : [];
  const picked = pickedWritePagesTool();
  const tool = tools.find((candidate) => candidate.family === picked) ?? tools[0] ?? null;
  const command = useAskAiCommand("steep", tool?.family);
  const release = useMutation({
    mutationFn: () => releaseSetAsideMaterial(groups.flatMap((group) => group.items.map((item) => item.item_id))),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: SET_ASIDE_QUERY_KEY });
      void queryClient.invalidateQueries({ queryKey: ["newMaterial"] });
    },
  });
  return (
    <p className="home-night-line" data-testid="home-unwritten">
      <span className="home-night-label">{t("pages.overview.unwritten")}</span>
      <span className="home-night-names">{namesLine(t, groups.map((group) => group.name))}</span>
      {/* Until the connected tools are known, no button: offering "Try
          again" and swapping it for the tool a moment later would be worse. */}
      {!agents.isSuccess ? null : tool && command ? (
        <AskInToolButton
          family={tool.family}
          tool={tool.name}
          text={unwrittenPrompt(t, command, groups)}
          label={t("pages.overview.unwrittenAsk", { tool: tool.name })}
          className="home-night-action"
          testId="home-unwritten-action"
        />
      ) : (
        <button
          type="button"
          className="home-night-action"
          data-testid="home-unwritten-action"
          title={t("pages.overview.unwrittenRetryTitle")}
          disabled={release.isPending}
          onClick={() => release.mutate()}
        >
          {t("pages.overview.unwrittenRetry")}
        </button>
      )}
    </p>
  );
}

/** How long after following AI tools' memory is turned on home says so. */
const AI_MEMORY_LINE_SECS = 24 * 60 * 60;
const AI_MEMORY_SEEN_KEY = "home:aiMemoryLineSeen";

function aiMemoryLineSeen(): number | null {
  try {
    const raw = localStorage.getItem(AI_MEMORY_SEEN_KEY);
    return raw ? Number(raw) : null;
  } catch {
    return null;
  }
}

/**
 * "Following what your AI tools remember": said once, the day following is
 * turned on, until the person looks or stops it. Afterwards what comes in
 * goes quietly; no count (UX-HOME-2).
 */
function AiMemoryLine({ at, onDone }: { readonly at: number; readonly onDone: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [looking, setLooking] = useState(false);
  const done = () => {
    try {
      localStorage.setItem(AI_MEMORY_SEEN_KEY, String(at));
    } catch {
      // Storage refused: the line goes for this visit only.
    }
    onDone();
  };
  const stop = useMutation({
    mutationFn: stopAiMemoryImport,
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["aiMemoryImport"] });
      done();
    },
  });
  return (
    <p className="home-night-line" data-testid="home-ai-memory">
      <span className="home-night-label">{t("pages.overview.aiMemoryFollowing")}</span>
      <button type="button" className="home-night-action" onClick={() => setLooking(true)}>{t("pages.overview.aiMemoryLook")}</button>
      <button type="button" className="home-night-action" disabled={stop.isPending} onClick={() => stop.mutate()}>{t("pages.overview.aiMemoryStop")}</button>
      {looking ? <AiMemoryDialog onClose={() => { setLooking(false); done(); }} /> : null}
    </p>
  );
}

/**
 * Home's first lines (UX-HOME-1): what last night steeped, across every Space
 * and the Wiki, so it sits above them rather than beside one heading, and
 * what Automatic set aside unwritten (UX-HOME-7). Each names, never counts
 * (UX-HOME-2), and is absent when it has nothing. Replay is a visible word,
 * not a hidden moon. Other quiet lines about the background belong in this
 * same block, one per line. `steeped={false}` leaves the steeped line to
 * "What you're working on", which carries its own.
 */
export function HomeNightLines({ onReplayNight, steeped: withSteeped = true }: { readonly onReplayNight?: () => void; readonly steeped?: boolean }) {
  const { t } = useTranslation();
  const things = useQuery({ queryKey: THINGS_QUERY_KEY, queryFn: listThings, refetchInterval: 60_000, enabled: withSteeped });
  const setAside = useSetAside();
  const steepedAll = withSteeped
    ? (things.data ?? []).filter((thing) => thing.title?.trim() && thing.dreamed_at && steepedToday(thing.dreamed_at))
    : [];
  const unwritten = (setAside.data?.groups ?? []).filter((group) => group.name.trim() && group.items.length > 0);
  const aiMemory = useQuery({ queryKey: ["aiMemoryImport"], queryFn: getAiMemoryImport, retry: false });
  const [seen, setSeen] = useState(aiMemoryLineSeen);
  const followingAt = aiMemory.data?.import?.at ?? null;
  const aiMemoryLine = followingAt !== null && followingAt !== seen && Date.now() / 1000 - followingAt < AI_MEMORY_LINE_SECS;
  if (steepedAll.length === 0 && unwritten.length === 0 && !aiMemoryLine) return null;
  const overnight = ranOvernight(steepedAll.map((thing) => thing.dreamed_at ?? 0));
  return (
    <div className="home-night" data-testid="home-night">
      {steepedAll.length > 0 ? (
        <p className="home-night-line" data-testid="home-steeped">
          <SteepMoon />
          <span className="home-night-label">{t(overnight ? "pages.overview.steepedOvernight" : "pages.overview.steepedToday")}</span>
          <span className="home-night-names">{namesLine(t, steepedAll.map((thing) => thing.title ?? ""))}</span>
          {onReplayNight ? (
            <button type="button" className="home-night-action" title={t("pages.overview.steepedReplay")} onClick={onReplayNight}>
              {t("pages.overview.steepedReplayShort")}
            </button>
          ) : null}
        </p>
      ) : null}
      {unwritten.length > 0 ? <UnwrittenLine groups={unwritten} /> : null}
      {aiMemoryLine && followingAt !== null ? <AiMemoryLine at={followingAt} onDone={() => setSeen(followingAt)} /> : null}
    </div>
  );
}
