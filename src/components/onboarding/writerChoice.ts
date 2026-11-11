// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Who turns material into pages, picked once on the welcome screen.
 * "local" is carried into the app like a start intent: home opens the steep
 * settings once, where the person picks a model and presses to download it
 * (nothing downloads from the welcome screen itself). "tool" and "none" need
 * nothing after the welcome screen: the tool's plugin writes pages in the
 * person's own next conversation, and "none" only stores.
 */
export type WriterChoice = "tool" | "local" | "none";

const KEY = "onboarding:writerLocal";

export function rememberLocalWriter() {
  try {
    sessionStorage.setItem(KEY, "1");
  } catch {
    // Storage refused: home simply does not open the settings by itself.
  }
}

/** True once, on the first home after "local" was picked. */
export function takeLocalWriter(): boolean {
  try {
    const set = sessionStorage.getItem(KEY) === "1";
    sessionStorage.removeItem(KEY);
    return set;
  } catch {
    return false;
  }
}

/** The tools that can write pages from waiting material, best first: their
 *  Wenlan plugin carries the steep workflow. Claude Code's also writes a few
 *  waiting pages at the start of each conversation. */
export const PAGE_WRITER_CLIENTS = ["claude_code", "codex_cli"] as const;
