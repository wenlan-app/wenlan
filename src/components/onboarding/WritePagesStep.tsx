// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ChatsCircle, Eye, FolderSimplePlus } from "@phosphor-icons/react";
import type { Page } from "../../lib/tauri";
import { waitingLine, workingHeadline, WrittenPages, type WritePagesState } from "../memory/askAi/WritePages";

interface WritePagesStepProps {
  /** Null outside the Wiki home, where nothing can be handed over. */
  readonly state: WritePagesState | null;
  readonly pages: readonly Page[];
  readonly done: boolean;
  /** Done only because the user skipped it, with nothing to bring in. */
  readonly skipped?: boolean;
  readonly onSkip?: (value: boolean) => void;
  readonly onAddSources?: () => void;
  readonly onImportChats?: () => void;
  readonly onSeeExample?: () => void;
  readonly onSelectPage?: (pageId: string) => void;
  /** Opens the steep settings, where the local model is chosen. */
  readonly onUseLocalModel?: () => void;
}

/**
 * The checklist's "turn your material into pages" row, laid out by the
 * checklist like its other rows. With nothing in yet it asks where the
 * user's material is (a folder of any kind, past AI chats, or the sample for
 * someone with none); once material is in, from an import or from what AI
 * tools saved, it hands it to the AI tool; then the same run the home line
 * follows shows here, so the pages appear in this row as they land.
 */
export function WritePagesStep({
  state,
  pages,
  done,
  skipped = false,
  onSkip,
  onAddSources,
  onImportChats,
  onSeeExample,
  onSelectPage,
  onUseLocalModel,
}: WritePagesStepProps) {
  const { t } = useTranslation();
  const view = state?.loaded ? state.stepView : { kind: "hidden" as const };
  const hasPages = pages.some((page) => page.creation_kind === "distilled" && page.status !== "draft");
  // Skipping was for having nothing to bring in. Once there is, the step
  // comes back with its button.
  const comeBack = skipped && !hasPages && view.kind === "ready";
  useEffect(() => {
    if (comeBack) onSkip?.(false);
  }, [comeBack, onSkip]);
  const title = (
    <p className="getting-started-step-title">
      {t("firstUse.checklist.steps.pages.title")}
      {done ? <span className="sr-only">{` ${t("firstUse.checklist.done")}`}</span> : null}
    </p>
  );
  const list = (rows: readonly Page[], writing: boolean) =>
    onSelectPage ? <WrittenPages rows={rows} writing={writing} onSelectPage={onSelectPage} /> : null;

  if (state && view.kind === "working") {
    const headline = workingHeadline(state, view, t);
    const hint = view.stuck ? t("askAi.writePages.stuck", { tool: state.toolName }) : null;
    return (
      <>
        <div className="getting-started-copy">
          {title}
          <p className="getting-started-step-body">{headline}</p>
          {hint ? <p className="getting-started-step-body">{hint}</p> : null}
          {list(view.written, view.writing)}
        </div>
        {state.run?.handedOver === "copied" ? (
          <button
            type="button"
            className="getting-started-action"
            disabled={state.command === null}
            onClick={() => void state.copyCommand()}
          >
            {state.copied ? t("askAi.copied") : t("askAi.writePages.copyCommand")}
          </button>
        ) : null}
      </>
    );
  }

  if (state && view.kind === "done") {
    return (
      <div className="getting-started-copy">
        {title}
        <p className="getting-started-step-body">
          {t("askAi.writePages.done", { count: view.written.length, minutes: view.minutes })}
        </p>
        {list(view.written, false)}
      </div>
    );
  }

  if (done && !comeBack) return <div className="getting-started-copy">{title}</div>;

  if (state && view.kind === "ready") {
    const { tool } = state;
    return (
      <>
        <div className="getting-started-copy">
          {title}
          <p className="getting-started-step-body">
            {tool ? waitingLine(state, pages, t) : t("askAi.writePages.noTool")}
          </p>
        </div>
        {tool ? (
          <button
            type="button"
            className="getting-started-action"
            title={state.opensTool
              ? t("askAi.writePages.opensTool", { tool: tool.name })
              : t("askAi.writePages.copiesCommand", { tool: tool.name })}
            disabled={state.command === null}
            onClick={() => void state.start()}
          >
            {state.opensTool
              ? t("askAi.writePages.askIn", { tool: tool.name })
              : t("askAi.writePages.copyCommand")}
          </button>
        ) : onUseLocalModel ? (
          <button type="button" className="getting-started-action" onClick={onUseLocalModel}>
            {t("askAi.writePages.useLocalModel")}
          </button>
        ) : null}
      </>
    );
  }

  // Nothing to hand over yet (or Wenlan writes the pages itself): where is
  // the user's material?
  const choices = [
    { key: "folder", icon: <FolderSimplePlus weight="regular" />, onClick: onAddSources },
    { key: "chats", icon: <ChatsCircle weight="regular" />, onClick: onImportChats },
    { key: "sample", icon: <Eye weight="regular" />, onClick: onSeeExample },
  ] as const;
  return (
    <div className="getting-started-copy">
      {title}
      <p className="getting-started-step-body">
        {state?.onDemand === false
          ? t("firstUse.checklist.steps.pages.automatic")
          : t("firstUse.checklist.steps.pages.body")}
      </p>
      <div className="getting-started-choices">
        {choices.map((choice) => choice.onClick ? (
          <button
            key={choice.key}
            type="button"
            className="getting-started-choice"
            data-testid={`getting-started-pages-${choice.key}`}
            onClick={choice.onClick}
          >
            <span className="getting-started-choice-icon" aria-hidden="true">{choice.icon}</span>
            <span className="getting-started-choice-copy">
              <strong>{t(`firstUse.checklist.steps.pages.${choice.key}.title`)}</strong>
              <span>{t(`firstUse.checklist.steps.pages.${choice.key}.hint`)}</span>
            </span>
          </button>
        ) : null)}
      </div>
      {onSkip ? (
        <p className="getting-started-step-body">
          <button type="button" className="getting-started-link" onClick={() => onSkip(true)}>
            {t("firstUse.checklist.steps.pages.skip")}
          </button>
        </p>
      ) : null}
    </div>
  );
}
