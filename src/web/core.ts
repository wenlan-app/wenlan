// SPDX-License-Identifier: AGPL-3.0-only
// Production browser implementation of @tauri-apps/api/core.

import { liveInvoke } from "./live-invoke";

export async function invoke(
  command: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  return liveInvoke(command, args);
}

export function convertFileSrc(path: string): string {
  return path;
}

export const isTauri = () => false;
export class Resource {
  close() {}
}
export class Channel {
  onmessage: ((message: unknown) => void) | null = null;
}
export class PluginListener {
  unregister() {}
}
export async function addPluginListener(): Promise<PluginListener> {
  return new PluginListener();
}
export function transformCallback(): number {
  return 0;
}
