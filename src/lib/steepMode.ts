// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { getBackgroundAiEnabled } from "./tauri";

/** What the user types in their AI tool to finish all pending steeping. */
export const STEEP_COMMAND = "/wenlan steep";

/**
 * Steeping has two modes, stored in `background_ai_enabled`: Automatic
 * (true, Wenlan steeps in the background) and On demand (false, the user's AI
 * tool steeps as it uses Wenlan). Never "off". Undefined until the setting is
 * read, so callers keep their Automatic copy rather than guessing.
 */
export function useSteepOnDemand(): boolean | undefined {
  // Called through an arrow so a missing command (an older shell, a test mock)
  // is a failed read, which keeps the Automatic copy, not a render error.
  const query = useQuery({ queryKey: ["backgroundAiEnabled"], queryFn: () => getBackgroundAiEnabled() });
  return query.isSuccess ? !query.data : undefined;
}

/**
 * "Refresh page" asked the daemon to rebuild a page and it had no model to do
 * it with (`status: skipped`). The card stays: the user's AI tool does the
 * update On demand, so treating this as done would hide work nobody did.
 */
export class SteepNeedsAiToolError extends Error {
  constructor() {
    super("steep_needs_ai_tool");
    this.name = "SteepNeedsAiToolError";
  }
}
