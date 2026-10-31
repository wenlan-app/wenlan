// SPDX-License-Identifier: AGPL-3.0-only
import { liveInvoke } from "./live-invoke";
import { allowsDevLiveCommand } from "../devLivePolicy";
export { convertFileSrc, isTauri, Resource, Channel, PluginListener, addPluginListener, transformCallback } from "./core";

export async function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
  // Window visibility is local to this separate preview, not a daemon write.
  if (command === "set_traffic_lights_visible") return null;
  // No native global shortcuts exist in this browser; do not forward to the daemon.
  if (command === "set_editor_composing") return null;
  // No wenlan://pair link can reach a browser preview.
  if (command === "take_remote_pairing_link") return null;
  if (command === "take_page_link") return null;
  // Nor can Web access stop on its own here, so no notice is ever waiting.
  if (command === "take_remote_access_notice") return null;
  if (!allowsDevLiveCommand(command)) {
    throw new Error("Wenlan Dev is read-only. Use the installed app to make changes.");
  }
  return liveInvoke(command, args);
}
