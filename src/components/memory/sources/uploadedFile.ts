// SPDX-License-Identifier: AGPL-3.0-only
/** The file path inside an indexed document's id, when that file is one the
 *  person uploaded (absolute under the uploads folder, or `sources/…` relative
 *  to the knowledge home the uploads folder sits in). */
export function uploadedFilePath(sourceId: string, folder: string | undefined): string | null {
  if (!folder) return null;
  const at = sourceId.indexOf("::");
  if (at < 0) return null;
  const path = sourceId.slice(at + 2).replace(/\\/g, "/");
  const root = folder.replace(/\\/g, "/").replace(/\/+$/, "");
  const name = root.split("/").pop() ?? "sources";
  if (path.toLowerCase().startsWith(`${root.toLowerCase()}/`)) return path;
  if (!/^([a-z]:)?\//i.test(path) && path.startsWith(`${name}/`)) return path;
  return null;
}
