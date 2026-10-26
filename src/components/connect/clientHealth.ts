// SPDX-License-Identifier: AGPL-3.0-only
//
// What the daemon-side checks say about one client beyond "is Wenlan in its
// config": whether the program is really there (`install_state`) and whether
// the raw entry would launch (`entry_health`). Both fields are optional on the
// wire type until every mock supplies them, and a client without them is
// described exactly as it was before they existed: every helper here answers
// "nothing to say" for a missing field.
//
// The Rust side's `detail` and `error` strings are English-only. Settings maps
// every repair reason, including reasons added by newer daemons, to one generic
// localized status and keeps the raw detail behind Copy details.

import type { McpClient, McpRepairReason } from "../../lib/tauri";

/** The reason a client's own `wenlan` entry would not launch, or `null` when
 *  it would, when there is no entry, or when the check did not run. */
export function repairReasonOf(client: Pick<McpClient, "entry_health">): McpRepairReason | null {
  const health = client.entry_health;
  return health?.kind === "needs_repair" ? health.reason : null;
}

/** The entry could not be read at all, so nothing is known about whether it
 *  launches. This is not a repair: there is nothing to fix from a failed look,
 *  so no action is offered. */
export function entryUnreadable(client: Pick<McpClient, "entry_health">): boolean {
  return client.entry_health?.kind === "unreadable";
}

/** The client's settings or home folder exist but no program was found. */
export function isConfigOnly(client: Pick<McpClient, "install_state">): boolean {
  return client.install_state?.kind === "config_only";
}
