// SPDX-License-Identifier: AGPL-3.0-only
import { createContext, useContext, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { CheckCircle, CircleNotch, Sparkle, X } from "@phosphor-icons/react";
import { getOnDeviceModel, getResolvedRouting } from "../../../lib/tauri";
import { chooseSteepMode, useSteepOnDemand } from "../../../lib/steepMode";
import { ANTHROPIC_MODELS } from "../../intelligence/IntelligenceSetup";
import { useLibraryInventory } from "../pages/useLibraryInventory";
import {
  copyAgainLabel,
  PAGES_PER_RUN,
  useWritePages,
  workingHeadline,
  WrittenPages,
  type WritePagesState,
  type WritePagesView,
} from "./WritePages";
import { ToolPicker } from "./ToolPicker";
import "./turnIntoPages.css";

/**
 * Where the import finish can send the user. The main window provides it, so
 * every import surface inside it (the Import page, the first-run import, the
 * import progress panel, the chat import dialog) offers the same hand-off
 * without each threading four callbacks through. Outside the main window
 * (the setup wizard) there is no provider, and the surfaces keep their plain
 * handoff sentence.
 */
export interface OrganizeActions {
  readonly onConnectTool: () => void;
  /** Settings → Intelligence: the mode control and the model picker. */
  readonly onOpenIntelligence: () => void;
  readonly onOpenActivity: () => void;
  readonly onSelectPage: (pageId: string) => void;
}

export const OrganizeActionsContext = createContext<OrganizeActions | null>(null);

export function useOrganizeActions(): OrganizeActions | null {
  return useContext(OrganizeActionsContext);
}

/** What the import brought in, for the lead line's noun. */
export type ImportedKind = "note" | "chat" | "file";

interface ImportOrganizeProps {
  readonly count: number;
  readonly kind: ImportedKind;
  /** Called after the card sends the user somewhere else, so a dialog it
   *  sits in can close. */
  readonly onLeave?: () => void;
  /** Inside a dialog: no box of its own. */
  readonly bare?: boolean;
  /** Where the lead line goes when the caller places it itself. */
  readonly lead?: "inline" | "none";
  /** Told whether the card shows a button to press, so the screen's own
   *  primary button can step back while it does. */
  readonly onActionChange?: (hasAction: boolean) => void;
  /** Called when the user puts the card away with "Later". */
  readonly onLater?: () => void;
}

/** "12 notes imported." Under a heading that already gives the count,
 *  `short` says nothing more. */
export function ImportedLead({
  count,
  kind,
  className,
  short = false,
}: {
  readonly count: number;
  readonly kind: ImportedKind;
  readonly className?: string;
  readonly short?: boolean;
}) {
  const { t } = useTranslation();
  if (short) return null;
  return (
    <p className={className ?? "turn-pages-lead"} data-testid="import-lead">
      {t("askAi.writePages.importCard.lead", { items: t(`askAi.writePages.kinds.${kind}`, { count }) })}
    </p>
  );
}

/**
 * The import finish: the lead line, then what turns the material into pages.
 * On demand, the "Turn them into pages" card hands it to the user's AI tool.
 * Automatic, one line names the model doing it. Until the mode is read, only
 * the lead line shows, so nothing claims the wrong mode.
 */
export function ImportOrganize({ count, kind, onLeave, bare = false, lead = "inline", onActionChange, onLater }: ImportOrganizeProps) {
  const onDemand = useSteepOnDemand();
  // Automatic (or not yet known) has no button of its own.
  useEffect(() => {
    if (onDemand !== true) onActionChange?.(false);
  }, [onDemand, onActionChange]);
  return (
    <div className="turn-pages-finish" data-testid="import-organize">
      {lead === "inline" ? <ImportedLead count={count} kind={kind} /> : null}
      {onDemand === true ? <TurnIntoPagesCard onLeave={onLeave} bare={bare} onActionChange={onActionChange} onLater={onLater} /> : null}
      {onDemand === false ? <AutomaticLine onLeave={onLeave} /> : null}
    </div>
  );
}

/** A display name for the model writing pages, or null when none is known
 *  by a name a person would recognise. */
function useAutomaticModel(): { blocked: boolean; name: string | null } {
  const routing = useQuery({ queryKey: ["resolvedRouting"], queryFn: getResolvedRouting });
  const route = routing.data?.synthesis;
  const onDevice = useQuery({
    queryKey: ["onDeviceModel"],
    queryFn: getOnDeviceModel,
    enabled: route?.source === "on_device",
  });
  if (!route) return { blocked: false, name: null };
  if (route.mode !== "pinned") return { blocked: true, name: null };
  if (route.source === "on_device") {
    const id = route.model ?? onDevice.data?.loaded ?? null;
    return { blocked: false, name: onDevice.data?.models.find((model) => model.id === id)?.display_name ?? null };
  }
  if (route.source === "anthropic") {
    return { blocked: false, name: ANTHROPIC_MODELS.find((model) => model.id === route.model)?.label ?? null };
  }
  if (route.source === "external") return { blocked: false, name: route.model?.trim() || null };
  return { blocked: false, name: null };
}

function AutomaticLine({ onLeave }: { readonly onLeave?: () => void }) {
  const { t } = useTranslation();
  const actions = useOrganizeActions();
  const model = useAutomaticModel();
  const go = (action: (() => void) | undefined) => () => {
    action?.();
    onLeave?.();
  };
  // Pages need a model. Without one, say so rather than promise pages.
  if (model.blocked) {
    return (
      <p className="turn-pages-auto" data-testid="turn-pages-automatic" data-state="blocked">
        <span>{t("firstUse.live.needsIntelligence")}</span>{" "}
        {actions ? (
          <button type="button" className="turn-pages-link" onClick={go(actions.onOpenIntelligence)}>
            {t("firstUse.live.intelligenceAction")}
          </button>
        ) : null}
      </p>
    );
  }
  return (
    <p className="turn-pages-auto" data-testid="turn-pages-automatic">
      <span>{t("askAi.writePages.importCard.automatic", { model: model.name ?? t("askAi.writePages.importCard.chosenModel") })}</span>{" "}
      {actions ? (
        <button type="button" className="turn-pages-link" onClick={go(actions.onOpenActivity)}>
          {t("askAi.writePages.importCard.seeActivity")}
        </button>
      ) : null}
    </p>
  );
}

type CardKind = "hidden" | "ready" | "noTool" | "working" | "done";

/** A run this much older than the screen belongs to earlier material. Long
 *  enough that leaving and coming back mid-run keeps following it. */
const EARLIER_RUN_MS = 30 * 60_000;

/** Which face the card shows. A run from well before this screen opened
 *  belongs to earlier material: the card offers the new material instead. */
export function turnIntoPagesKind(
  state: Pick<WritePagesState, "loaded" | "onDemand" | "run" | "tool" | "waitingTotal"> & { readonly cardView: WritePagesView },
  openedAt: number,
): CardKind {
  if (!state.loaded || state.onDemand !== true) return "hidden";
  const view = state.cardView;
  const earlierRun = state.run !== null && state.run.startedAt < openedAt - EARLIER_RUN_MS;
  // An earlier run is followed only while its tool is still writing; one
  // that never started (the command was never pasted) is left behind.
  if (view.kind === "working" && (!earlierRun || view.writing)) return "working";
  if (view.kind === "done" && !earlierRun) return "done";
  if (view.kind === "hidden" && !earlierRun) return "hidden";
  if (state.waitingTotal < 1) return "hidden";
  return state.tool ? "ready" : "noTool";
}

interface TurnIntoPagesCardProps {
  readonly onLeave?: () => void;
  readonly bare?: boolean;
  readonly onActionChange?: (hasAction: boolean) => void;
  readonly onLater?: () => void;
}

/**
 * "Turn them into pages": the import finish's hand-off to the user's AI
 * tool. It runs on the same state as the home line and the checklist step
 * (`useWritePages`), so a press here is the run those follow, and the pages
 * land in its list as the tool writes them.
 */
export function TurnIntoPagesCard({ onLeave, bare = false, onActionChange, onLater }: TurnIntoPagesCardProps) {
  const { pages } = useLibraryInventory();
  const state = useWritePages(pages);
  const hasPages = pages.some((page) => page.creation_kind === "distilled" && page.status !== "draft");
  return <TurnIntoPagesView state={state} hasPages={hasPages} onLeave={onLeave} bare={bare} onActionChange={onActionChange} onLater={onLater} />;
}

export function TurnIntoPagesView({
  state,
  hasPages,
  onLeave,
  bare = false,
  onActionChange,
  onLater,
}: {
  readonly state: WritePagesState;
  readonly hasPages: boolean;
  readonly onLeave?: () => void;
  readonly bare?: boolean;
  readonly onActionChange?: (hasAction: boolean) => void;
  readonly onLater?: () => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const actions = useOrganizeActions();
  const openedAt = useRef(Date.now()).current;
  const titleId = useId();
  const [later, setLater] = useState(false);
  const [explaining, setExplaining] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [switchFailed, setSwitchFailed] = useState(false);

  const kind = turnIntoPagesKind(state, openedAt);
  // A face with a primary button: the offer, the Automatic explanation, the
  // connect prompt, and "Write 8 more" while notes still wait.
  const view = state.cardView;
  const hasAction = !later && kind !== "hidden" && (
    explaining
    || kind === "ready"
    || (kind === "noTool" && actions !== null)
    || (kind === "done" && view.kind === "done" && view.left > 0 && state.command !== null)
  );
  useEffect(() => {
    onActionChange?.(hasAction);
  }, [hasAction, onActionChange]);
  useEffect(() => () => onActionChange?.(false), [onActionChange]);
  if (kind === "hidden" || later) return null;

  const go = (action: (() => void) | undefined) => () => {
    action?.();
    onLeave?.();
  };
  const selectPage = (pageId: string) => {
    actions?.onSelectPage(pageId);
    onLeave?.();
  };
  const toolName = state.tool?.name ?? state.run?.tool ?? null;
  const inlineTool = toolName ?? t("askAi.writePages.importCard.yourTool");

  const putOff = () => {
    setLater(true);
    onLater?.();
  };
  // An offer the user can put off closes from its corner, so the card's own
  // row holds only the one thing to do.
  const shell = (body: ReactNode, cardState: string, closable = false) => (
    <section
      className={`turn-pages${bare ? " turn-pages--bare" : ""}`}
      data-testid="turn-into-pages"
      data-state={cardState}
      aria-labelledby={titleId}
      aria-live="polite"
    >
      <div className="turn-pages-head">
      <h3 id={titleId} className="turn-pages-title">
        {cardState === "done" ? (
          <CheckCircle aria-hidden="true" weight="fill" className="turn-pages-icon turn-pages-icon--done" />
        ) : cardState === "working" ? (
          <CircleNotch aria-hidden="true" weight="bold" className="turn-pages-icon write-pages-spin" />
        ) : (
          <Sparkle aria-hidden="true" weight="regular" className="turn-pages-icon" />
        )}
        {t("askAi.writePages.importCard.title")}
      </h3>
      {closable ? (
        <button
          type="button"
          className="turn-pages-close"
          data-testid="turn-into-pages-later"
          aria-label={t("askAi.writePages.importCard.later")}
          title={t("askAi.writePages.importCard.later")}
          onClick={putOff}
        >
          <X aria-hidden="true" weight="regular" />
        </button>
      ) : null}
      </div>
      {body}
    </section>
  );

  if (explaining) {
    const confirm = async () => {
      if (switching) return;
      setSwitching(true);
      setSwitchFailed(false);
      try {
        await chooseSteepMode(queryClient, true);
        // The model is picked where the mode lives.
        go(actions?.onOpenIntelligence)();
      } catch {
        setSwitchFailed(true);
      } finally {
        setSwitching(false);
      }
    };
    return shell(
      <>
        <p className="turn-pages-body">{t("askAi.writePages.importCard.automaticExplain", { tool: inlineTool })}</p>
        <p className="turn-pages-hint">{t("askAi.writePages.importCard.automaticNote")}</p>
        {switchFailed ? <p className="turn-pages-error" role="alert">{t("askAi.writePages.importCard.switchFailed")}</p> : null}
        <div className="turn-pages-actions">
          <button type="button" className="turn-pages-primary" disabled={switching} onClick={() => void confirm()}>
            {t("askAi.writePages.importCard.confirmSwitch")}
          </button>
          <button type="button" className="turn-pages-link" disabled={switching} onClick={() => setExplaining(false)}>
            {t("askAi.writePages.importCard.keepOnDemand")}
          </button>
        </div>
      </>,
      "explain",
    );
  }

  // The other way to get pages, as one quiet line under the actions.
  const secondary = (
    <p className="turn-pages-alt">
      <button type="button" className="turn-pages-link" onClick={() => setExplaining(true)}>
        {t("askAi.writePages.importCard.switchToAutomatic")}
      </button>
    </p>
  );

  if (kind === "noTool") {
    return shell(
      <>
        <p className="turn-pages-body">{t("askAi.writePages.importCard.noTool")}</p>
        <div className="turn-pages-actions">
          {actions ? (
            <button type="button" className="turn-pages-primary" onClick={go(actions.onConnectTool)}>
              {t("askAi.writePages.connect")}
            </button>
          ) : null}
        </div>
        {secondary}
      </>,
      "no-tool",
      true,
    );
  }

  const startLabel = state.copied
    ? t("askAi.copied")
    : state.opensTool
      ? t("askAi.writePages.importCard.write", { tool: toolName })
      : t("askAi.writePages.importCard.copyFor", { tool: toolName });

  if (kind === "ready") {
    return shell(
      <>
        {/* The last run let its material go without a page: say so, rather
            than quietly offering the same button again. */}
        {state.stopped ? <p className="turn-pages-body" data-testid="turn-into-pages-stopped">{t("askAi.writePages.stopped")}</p> : null}
        <p className="turn-pages-body">
          {t(hasPages ? "askAi.writePages.importCard.bodyMore" : "askAi.writePages.importCard.body", {
            tool: toolName,
            pages: PAGES_PER_RUN,
          })}
        </p>
        {/* A tool the app cannot open gets the command to paste, and says so. */}
        {!state.opensTool && state.tool ? (
          <p className="turn-pages-hint">{t("askAi.writePages.copiesCommand", { tool: state.tool.name })}</p>
        ) : null}
        <div className="turn-pages-actions">
          <button
            type="button"
            className="turn-pages-primary"
            disabled={state.command === null}
            onClick={() => void state.start()}
          >
            {startLabel}
          </button>
          {state.tools.length > 1 && state.tool ? (
            <ToolPicker tools={state.tools} current={state.tool.family} onChoose={state.chooseTool} />
          ) : null}
        </div>
        {secondary}
      </>,
      "ready",
      true,
    );
  }

  if (view.kind === "working") {
    const hint = view.stuck ? t("askAi.writePages.stuck", { tool: state.toolName }) : null;
    const copyAgain = copyAgainLabel(state, view.written.length, t);
    return shell(
      <>
        <p className="turn-pages-body">{workingHeadline(state, view, t, PAGES_PER_RUN)}</p>
        <p className="turn-pages-hint">{t("askAi.writePages.importCard.workingAway")}</p>
        {hint ? <p className="turn-pages-hint">{hint}</p> : null}
        {copyAgain ? (
          <div className="turn-pages-actions">
            <button
              type="button"
              className="turn-pages-link"
              disabled={state.command === null}
              onClick={() => void state.copyCommand()}
            >
              {copyAgain}
            </button>
          </div>
        ) : null}
        <WrittenPages rows={view.written} writing={view.writing && !view.stuck} onSelectPage={selectPage} />
      </>,
      "working",
    );
  }

  if (view.kind !== "done") return null;
  const waiting = view.left > 0;
  return shell(
    <>
      <p className="turn-pages-body">
        {waiting
          ? t("askAi.writePages.importCard.doneWaiting", { count: view.written.length, waiting: state.waiting })
          : t("askAi.writePages.importCard.done", { count: view.written.length })}
      </p>
      <WrittenPages rows={view.written} writing={false} onSelectPage={selectPage} />
      {/* While anything waits, the next batch is one press away, even when
          the run was started outside the app and no tool is known. */}
      {waiting && state.command !== null ? (
        <div className="turn-pages-actions">
          <button
            type="button"
            className="turn-pages-primary"
            disabled={state.command === null}
            onClick={() => void state.start()}
          >
            {state.copied ? t("askAi.copied") : t("askAi.writePages.importCard.writeMore", { pages: PAGES_PER_RUN })}
          </button>
        </div>
      ) : null}
    </>,
    "done",
  );
}
