// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openToolWithPrompt, PROMPT_LINK_FAMILIES } from "../../../lib/tauri";
import { writeClipboard } from "../page/codeBasisShared";

/** How a hand-over ended: the tool opened with the text typed in, the text is
 *  on the clipboard to paste there, or neither worked. */
export type HandOver = "opened" | "copied" | "failed";

const SHOWN_MS = 4_000;

/** The plugin's front door in tools that have it; other tools take plain words. */
export function frontDoorFor(family: string): string | null {
  if (family === "claude-code") return "/wenlan";
  if (family === "codex") return "$wenlan";
  return null;
}

/** Whether the tool can be opened with text already typed in. */
export function opensWithText(family: string): boolean {
  return PROMPT_LINK_FAMILIES.includes(family);
}

/** Copies the text first, so it is there to paste whatever happens next, then
 *  opens the tool with it typed in where the tool allows that. */
export async function handToTool(family: string, text: string): Promise<HandOver> {
  const copied = await writeClipboard(text);
  if (opensWithText(family)) {
    try {
      await openToolWithPrompt(family, text);
      return "opened";
    } catch {
      // Not installed here, or nothing opens the link; the copy still works.
    }
  }
  return copied ? "copied" : "failed";
}

interface AskInToolButtonProps {
  readonly family: string;
  /** The tool's name as the user knows it, e.g. "Claude Code". */
  readonly tool: string;
  readonly text: string;
  /** The button's words while it opens the tool; it says "Copy" otherwise. */
  readonly label: string;
  readonly className?: string;
  readonly testId?: string;
}

/** One button that takes the user to their AI tool with the text ready. After
 *  a press it says what happened, so it never looks like nothing did. */
export function AskInToolButton({ family, tool, text, label, className, testId }: AskInToolButtonProps) {
  const { t } = useTranslation();
  const [result, setResult] = useState<HandOver | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  const words = result === "opened"
    ? t("askAi.handOver.opened", { tool })
    : result === "copied"
      ? t("askAi.handOver.copied", { tool })
      : result === "failed"
        ? t("askAi.handOver.failed")
        : opensWithText(family) ? label : t("askAi.copy");
  return (
    <button
      type="button"
      className={className ?? "ask-in-tool"}
      data-testid={testId}
      data-result={result ?? undefined}
      aria-live="polite"
      onClick={() => {
        void handToTool(family, text).then((next) => {
          setResult(next);
          if (timer.current) clearTimeout(timer.current);
          timer.current = setTimeout(() => setResult(null), SHOWN_MS);
        });
      }}
    >
      {words}
    </button>
  );
}
