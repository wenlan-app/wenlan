// SPDX-License-Identifier: AGPL-3.0-only
import type { Page } from "../../../lib/tauri";
import { isHowIWorkPage } from "../skills/howIWorkModel";
import { pageFolderPath } from "./pageInventory";

/** The three fixed sections of the left side. They are a way of looking at
 *  the one pages folder: nothing moves on disk and no folder has a stored
 *  kind. Rules and Skills are told by where a page already sits; Docs is
 *  everything else. */
export type PageSection = "docs" | "skills" | "rules";
export const PAGE_SECTIONS: readonly PageSection[] = ["docs", "skills", "rules"];

/** The folder names that make a page a rules page, in any app language.
 *  The same names the daemon reads (`skill_grow::RULES_FOLDER_NAMES`). */
export const RULES_FOLDER_NAMES: readonly string[] = ["Rules", "規則", "规则"];
/** The folder names whose pages show under Skills. Only this list reads
 *  them: the daemon gives a Skills folder no meaning of its own. */
export const SKILLS_FOLDER_NAMES: readonly string[] = ["Skills", "技能"];

function asciiLower(text: string): string {
  return text.replace(/[A-Z]/gu, (letter) => letter.toLowerCase());
}

function named(folder: string, names: readonly string[]): boolean {
  return names.some((name) => asciiLower(name) === asciiLower(folder));
}

/** The section a folder path belongs to: Rules or Skills when a folder of
 *  that name is on the path at any depth, Docs otherwise. `space` is the
 *  page's Space: a first folder that only carries the Space's name is the
 *  Space's own folder, not a rules folder. */
export function folderSection(path: string | null, space?: string | null): PageSection {
  const folders = (path ?? "").split(/[/\\]/u).filter(Boolean);
  const own = space?.trim();
  const inside = own && folders[0] === own ? folders.slice(1) : folders;
  if (inside.some((folder) => named(folder, RULES_FOLDER_NAMES))) return "rules";
  if (inside.some((folder) => named(folder, SKILLS_FOLDER_NAMES))) return "skills";
  return "docs";
}

/** Which section lists `page`. A "How I work" page is a rules page wherever
 *  its file is. */
export function pageSection(page: Pick<Page, "content" | "title" | "space" | "storage_path" | "folder_path" | "status">): PageSection {
  if (isHowIWorkPage(page)) return "rules";
  return folderSection(pageFolderPath(page as Page), page.space);
}

/** The first folder that carries one of `section`'s names, or null when the
 *  library has none yet. New pages of that section go there. */
export function sectionFolderPath(section: Exclude<PageSection, "docs">, folderPaths: readonly string[]): string | null {
  const names = section === "rules" ? RULES_FOLDER_NAMES : SKILLS_FOLDER_NAMES;
  const matches = folderPaths.filter((path) => named(path.split("/").pop() ?? "", names));
  return matches.sort((left, right) => left.split("/").length - right.split("/").length || left.localeCompare(right))[0] ?? null;
}
