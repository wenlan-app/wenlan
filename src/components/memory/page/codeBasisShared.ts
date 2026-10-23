// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { clipboardWrite, type CodeBasisRef } from "../../../lib/tauri";

const COPIED_MS = 2_000;

/** `path:line_start`, the place to look in an editor. This is what gets copied. */
export function codeBasisLocation(row: Pick<CodeBasisRef, "path" | "line_start">): string {
  return `${row.path}:${row.line_start}`;
}

/** What is shown for a row: `path:20–30`, or `path:23` when it is one line. */
export function codeBasisLocationLabel(row: Pick<CodeBasisRef, "path" | "line_start" | "line_end">): string {
  return row.line_end > row.line_start
    ? `${row.path}:${row.line_start}\u2013${row.line_end}`
    : codeBasisLocation(row);
}

/** The file on disk: the repository root and the relative path joined with "/". */
export function codeBasisAbsolutePath(row: Pick<CodeBasisRef, "repo_root" | "path">): string {
  return `${row.repo_root.replace(/[\\/]+$/, "")}/${row.path}`;
}

/** The wording of a span: "lines 3-9", or "line 3" when it is one line. */
export function codeBasisSpanVars(row: Pick<CodeBasisRef, "path" | "line_start" | "line_end">) {
  return { file: row.path, from: row.line_start, to: row.line_end };
}

/** The line a row gets in the notice: what changed, or what could not be found. */
export function codeBasisLineText(t: TFunction, row: CodeBasisRef): string {
  const vars = codeBasisSpanVars(row);
  if (row.state === "pending") {
    return t(row.line_start === row.line_end ? "codeBasis.changedLineOne" : "codeBasis.changedLine", vars);
  }
  if (row.state === "unavailable") {
    if (row.reason === "file_missing") return t("codeBasis.missingFile", vars);
    if (row.reason === "file_deleted") return t("codeBasis.deletedFile", vars);
    if (row.reason === "repo_missing") return t("codeBasis.missingRepo", vars);
    return t("codeBasis.unreadable", vars);
  }
  return "";
}

/**
 * What the background check concluded about a changed row, for the second line
 * of the notice: its sentence as sent, plus "next steep fixes it" when it
 * judged the note stale. Null when there is nothing to say.
 */
export function codeBasisAiLineText(t: TFunction, row: CodeBasisRef): string | null {
  if (row.state !== "pending") return null;
  if (row.reason === "lines_not_found") return t("codeBasis.linesLost");
  const sentence = row.explanation?.trim();
  if (!sentence) return null;
  if (row.ai_verdict !== "stale") return t("codeBasis.aiLooked", { sentence });
  const suffix = t("codeBasis.aiStaleSuffix");
  if (sentence.endsWith(suffix)) return t("codeBasis.aiLooked", { sentence });
  const bare = sentence.replace(/[.。!！]+$/u, "");
  return t("codeBasis.aiLooked", { sentence: bare.endsWith(suffix) ? bare : `${bare}${suffix}` });
}

/**
 * The quiet word on a changed row in the info panel, where nothing is asked of
 * the person: what the check concluded, that the next steep will look, or that
 * it updates the page for a deleted file. Null otherwise, or when no model
 * will look at a changed row.
 */
export function codeBasisQuietNoteText(t: TFunction, row: CodeBasisRef, modelAvailable: boolean): string | null {
  if (row.state === "unavailable" && row.reason === "file_deleted") return t("codeBasis.deletedWillFix");
  if (row.state !== "pending") return null;
  const judged = codeBasisAiLineText(t, row);
  if (judged) return judged;
  return modelAvailable && !row.ai_verdict ? t("codeBasis.aiWillLook") : null;
}

/** Copy through the app, falling back to the web API outside Tauri. */
export async function writeClipboard(text: string): Promise<boolean> {
  try {
    await clipboardWrite(text);
    return true;
  } catch {
    // Outside Tauri (browser preview) the plugin is missing; try the web API.
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Copy a text and report "copied" for two seconds. */
export function useCopyLocation() {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  const copy = useCallback(async (text: string) => {
    if (!(await writeClipboard(text))) return;
    setCopied(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), COPIED_MS);
  }, []);
  return { copied, copy };
}
