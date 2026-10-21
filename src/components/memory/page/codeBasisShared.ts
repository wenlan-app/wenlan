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
    if (row.reason === "repo_missing") return t("codeBasis.missingRepo", vars);
    return t("codeBasis.unreadable", vars);
  }
  return "";
}

async function writeClipboard(text: string): Promise<boolean> {
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
