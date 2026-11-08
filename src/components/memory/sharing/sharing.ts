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

const CLOUD_ROOTS: readonly { cloud: Cloud; match: RegExp }[] = [
  { cloud: "googleDrive", match: /[\\/](?:Library[\\/]CloudStorage[\\/]GoogleDrive-[^\\/]+(?:[\\/](?:My Drive|我的雲端硬碟|我的云端硬盘|Shared drives|共用雲端硬碟|共享云端硬盘))?|Google Drive(?:[\\/](?:My Drive|我的雲端硬碟|我的云端硬盘))?)(?=[\\/]|$)/ },
  { cloud: "icloud", match: /[\\/]Library[\\/]Mobile Documents[\\/]com~apple~CloudDocs(?=[\\/]|$)/ },
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
