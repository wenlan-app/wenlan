// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import type { Page } from "../../lib/tauri";
import { waitingLine, WrittenPages, type WritePagesState } from "../memory/askAi/WritePages";

interface WritePagesStepProps {
  /** Null outside the Wiki home, where nothing can be handed over. */
  readonly state: WritePagesState | null;
  readonly pages: readonly Page[];
  readonly done: boolean;
  readonly onSelectPage?: (pageId: string) => void;
  /** Opens the steep settings, where the local model is chosen. */
  readonly onUseLocalModel?: () => void;
}

/**
 * The checklist's "turn your material into pages" row: the copy and the one
 * action, laid out by the checklist like its other rows. The same run the
 * home line follows shows here while the checklist is open, so the pages the
 * tool writes appear in this row as they land.
 */
export function WritePagesStep({ state, pages, done, onSelectPage, onUseLocalModel }: WritePagesStepProps) {
  const { t } = useTranslation();
  const view = state?.loaded ? state.stepView : { kind: "hidden" as const };
  const title = (
    <p className="getting-started-step-title">
      {t("firstUse.checklist.steps.pages.title")}
      {done ? <span className="sr-only">{` ${t("firstUse.checklist.done")}`}</span> : null}
    </p>
  );
  const list = (rows: readonly Page[], writing: boolean) =>
    onSelectPage ? <WrittenPages rows={rows} writing={writing} onSelectPage={onSelectPage} /> : null;

  if (state && view.kind === "working") {
    const hint = view.stuck
      ? t("askAi.writePages.stuck", { tool: state.toolName })
      : view.written.length === 0 && !view.writing
        ? state.run?.handedOver !== "opened"
          ? t("askAi.writePages.pasteIt", { tool: state.toolName })
          : t("askAi.writePages.pressEnter", { tool: state.toolName })
        : null;
    return (
      <>
        <div className="getting-started-copy">
          {title}
          <p className="getting-started-step-body">
            {view.written.length > 0
              ? t("askAi.writePages.workingCount", { tool: state.toolName, count: view.written.length })
              : t("askAi.writePages.working", { tool: state.toolName })}
            {hint ? <>{" "}{hint}</> : null}
          </p>
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

  if (done) return <div className="getting-started-copy">{title}</div>;

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

  // Nothing to hand over yet, or Wenlan writes the pages itself.
  return (
    <div className="getting-started-copy">
      {title}
      <p className="getting-started-step-body">
        {state?.onDemand === false
          ? t("firstUse.checklist.steps.pages.automatic")
          : t("firstUse.checklist.steps.pages.body")}
      </p>
    </div>
  );
}
