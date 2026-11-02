// SPDX-License-Identifier: AGPL-3.0-only
/**
 * "Report a problem" on the screen the app shows when it cannot reach its
 * background service. The app gathers the version, platform and log tail
 * (`problem_report`); this copies that text and opens a prefilled GitHub
 * issue, which the person reads before anything is sent.
 */
import { invoke } from "@tauri-apps/api/core";
import { open as openExternal } from "@tauri-apps/plugin-shell";

const NEW_ISSUE_URL = "https://github.com/wenlan-app/wenlan/issues/new";
/** GitHub refuses longer new-issue URLs; stay well under its limit. */
const MAX_URL_LENGTH = 7000;

/**
 * The prefilled new-issue URL. A body too long for a URL loses lines from
 * the top of its logs first: the newest lines say most about why it failed.
 */
export function issueUrl(title: string, body: string): string {
  const build = (text: string) =>
    `${NEW_ISSUE_URL}?title=${encodeURIComponent(title)}&body=${encodeURIComponent(text)}`;
  let text = body;
  let url = build(text);
  while (url.length > MAX_URL_LENGTH) {
    const fence = text.indexOf("```\n");
    const cut = fence < 0 ? -1 : text.indexOf("\n", fence + 4);
    if (cut < 0) {
      text = text.slice(0, Math.max(0, text.length - (url.length - MAX_URL_LENGTH)));
    } else {
      text = text.slice(0, fence + 4) + text.slice(cut + 1);
    }
    url = build(text);
  }
  return url;
}

/**
 * Copy the report and open the prefilled issue. Copying comes first, so a
 * person who cannot reach GitHub (offline, no account) still has it to paste.
 * Resolves `true` when the report was copied.
 */
export async function reportProblem(title: string): Promise<boolean> {
  const body = await invoke<string>("problem_report");
  let copied = false;
  try {
    await navigator.clipboard.writeText(body);
    copied = true;
  } catch {
    // Opening the issue below still carries the report.
  }
  try {
    await openExternal(issueUrl(title, body));
  } catch {
    // Offline or no browser: the copied report is the fallback.
  }
  return copied;
}
