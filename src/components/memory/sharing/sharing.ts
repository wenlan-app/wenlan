// SPDX-License-Identifier: AGPL-3.0-only
// Sharing a Space or one page with people through a cloud folder: the shares
// query, and how a folder and its members are named on screen.
import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { listShares, type SharedMember, type SharedStatus } from "../../../lib/tauri";

export const SHARES_KEY = ["shares"] as const;

/** Everything this Wenlan shares, refreshed while the app is open. */
export function useShares() {
  return useQuery({
    queryKey: SHARES_KEY,
    queryFn: () => listShares(),
    refetchInterval: 10_000,
    staleTime: 5_000,
  });
}

export function spaceShare(shares: readonly SharedStatus[] | undefined, space: string): SharedStatus | undefined {
  return shares?.find((share) => share.kind === "space" && share.target === space);
}

export function pageShare(shares: readonly SharedStatus[] | undefined, pageId: string): SharedStatus | undefined {
  return shares?.find((share) => share.kind === "page" && share.target === pageId);
}

/** The other members, by name. */
export function others(share: SharedStatus): SharedMember[] {
  return share.members.filter((member) => member.id !== share.member_id);
}

type Cloud = "googleDrive" | "icloud" | "dropbox" | "oneDrive";

// Google Drive for desktop on Windows is its own drive (G: by default):
// `G:\My Drive`, or the Drive's name in the person's language. iCloud for
// Windows keeps iCloud Drive in `C:\Users\<name>\iCloudDrive`.
const CLOUD_ROOTS: readonly { cloud: Cloud; match: RegExp }[] = [
  { cloud: "googleDrive", match: /[\\/](?:Library[\\/]CloudStorage[\\/]GoogleDrive-[^\\/]+(?:[\\/](?:My Drive|我的雲端硬碟|我的云端硬盘|Shared drives|共用雲端硬碟|共享云端硬盘))?|Google Drive(?:[\\/](?:My Drive|我的雲端硬碟|我的云端硬盘))?)(?=[\\/]|$)/ },
  { cloud: "googleDrive", match: /^[A-Za-z]:[\\/](?:My Drive|我的雲端硬碟|我的云端硬盘|Shared drives|共用雲端硬碟|共享云端硬盘)(?=[\\/]|$)/ },
  { cloud: "icloud", match: /[\\/]Library[\\/]Mobile Documents[\\/]com~apple~CloudDocs(?=[\\/]|$)/ },
  { cloud: "icloud", match: /[\\/]iCloudDrive(?=[\\/]|$)/ },
  { cloud: "dropbox", match: /[\\/](?:Library[\\/]CloudStorage[\\/])?Dropbox(?:[ -][^\\/]*)?(?=[\\/]|$)/ },
  { cloud: "oneDrive", match: /[\\/](?:Library[\\/]CloudStorage[\\/])?OneDrive(?:[ -][^\\/]*)?(?=[\\/]|$)/ },
];

function cloudOf(folder: string): { cloud: Cloud; rest: string } | null {
  for (const { cloud, match } of CLOUD_ROOTS) {
    const found = match.exec(folder);
    if (found) return { cloud, rest: folder.slice(found.index + found[0].length) };
  }
  return null;
}

/** The cloud service a folder syncs with, by name; `null` when it is in none
 *  this knows. */
export function cloudName(folder: string, t: TFunction): string | null {
  const found = cloudOf(folder);
  if (!found) return null;
  switch (found.cloud) {
    case "googleDrive": return "Google Drive";
    case "icloud": return t("sharing.icloud");
    case "dropbox": return "Dropbox";
    case "oneDrive": return "OneDrive";
  }
}

/** A folder as people know it: "Google Drive › Wedding" rather than the
 *  whole path. Outside a known cloud, its last two names. */
export function shortFolder(folder: string, t: TFunction): string {
  const parts = (path: string) => path.split(/[\\/]+/).filter(Boolean);
  const found = cloudOf(folder);
  if (found) {
    const rest = parts(found.rest);
    return [cloudName(folder, t), ...rest.slice(-2)].join(" › ");
  }
  return parts(folder).slice(-2).join(" › ") || folder;
}

/** The sentence under a shared Space's or page's title. */
export function withLine(share: SharedStatus, t: TFunction): string {
  const rest = others(share);
  if (rest.length === 0) return t("sharing.withAlone");
  if (rest.length === 1) return t("sharing.withOne", { name: rest[0].name });
  return t("sharing.withMany", { name: rest[0].name, count: rest.length - 1 });
}

/** Who wrote a page's last version, when it was another member: their name,
 *  or "<name>'s AI tool". `null` for this person and anyone else. */
export function sharedWriter(editedBy: string | null | undefined, shares: readonly SharedStatus[] | undefined, t: TFunction): string | null {
  const found = /^(shared_person|shared_ai):([A-Za-z0-9_-]+)$/.exec(editedBy ?? "");
  if (!found || !shares) return null;
  const [, kind, id] = found;
  for (const share of shares) {
    const member = share.members.find((candidate) => candidate.id === id);
    if (member) return kind === "shared_ai" ? t("sharing.memberAi", { name: member.name }) : member.name;
  }
  return null;
}

/** The button that opens the shared folder, named after the platform's own
 * file manager: Finder on macOS, File Explorer on Windows, and a plain
 * "Open folder" where the platform is Linux or cannot be told. */
export function revealFolderLabel(
  t: TFunction,
  ua: string | undefined = globalThis.navigator?.userAgent,
): string {
  if (ua && /\b(Macintosh|Mac OS X)\b/.test(ua)) return t("sharing.showInFinder");
  if (ua && /\bWindows\b/.test(ua)) return t("sharing.showInExplorer");
  return t("sharing.showInFolder");
}

/** True when a Space syncs through a team server, or this computer hosts it
 *  for a team. */
export function isTeamShare(share: SharedStatus | undefined): boolean {
  return Boolean(share && (share.host || share.hosted));
}

/** The share of a Space by its name, compared the way Space names are. */
export function shareOfSpace(shares: readonly SharedStatus[] | undefined, name: string): SharedStatus | undefined {
  const key = name.trim().toLocaleLowerCase();
  return shares?.find((share) => share.kind === "space" && share.target.trim().toLocaleLowerCase() === key);
}

export type TeamSpaceGroup<T> = {
  /** The server's address as people read it, or `null` for Spaces this
   *  computer hosts. */
  readonly host: string | null;
  readonly spaces: T[];
};

/** Splits Spaces into the person's own and one group per team server (then
 *  the Spaces this computer hosts). Order inside each is kept. */
export function groupTeamSpaces<T extends { name: string }>(
  spaces: readonly T[],
  shares: readonly SharedStatus[] | undefined,
): { own: T[]; groups: TeamSpaceGroup<T>[] } {
  const own: T[] = [];
  const byHost = new Map<string, T[]>();
  const hosted: T[] = [];
  for (const space of spaces) {
    const share = shareOfSpace(shares, space.name);
    if (share?.host) {
      const host = share.host.replace(/^[a-z][a-z0-9+.-]*:\/\//i, "").replace(/\/+$/, "");
      byHost.set(host, [...(byHost.get(host) ?? []), space]);
    } else if (share?.hosted) hosted.push(space);
    else own.push(space);
  }
  const groups: TeamSpaceGroup<T>[] = [...byHost].map(([host, list]) => ({ host, spaces: list }));
  if (hosted.length > 0) groups.push({ host: null, spaces: hosted });
  return { own, groups };
}
