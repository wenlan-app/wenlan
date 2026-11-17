// SPDX-License-Identifier: AGPL-3.0-only
import type { Page } from "../../../lib/tauri";
import { isHowIWorkPage, isSavedRulesPage } from "../skills/howIWorkModel";
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

/** Which folders are a Space's own (or a team's), so their names are not
 *  read as folders the person named. The daemon decides rules the same way
 *  (`space_folders::placed`): only the folders inside a Space's folder count.
 *  Empty while Spaces do not keep their pages in folders of their own. */
export type SpaceLayout = {
  /** Each Space's own folder, relative to the pages folder. */
  readonly spaceFolders: readonly string[];
  /** Each team's folder, which holds its Spaces' folders. */
  readonly teamFolders: readonly string[];
};
export const NO_SPACE_LAYOUT: SpaceLayout = { spaceFolders: [], teamFolders: [] };

/** Names compared as a file system does: any letter case, any Unicode form. */
function nameKey(text: string): string {
  return text.normalize("NFKC").toLowerCase();
}

function named(folder: string, names: readonly string[]): boolean {
  return names.some((name) => nameKey(name) === nameKey(folder));
}

function parts(path: string | null): string[] {
  return (path ?? "").split(/[/\\]/u).filter(Boolean);
}

/** A Space's folder carries the Space's name, or that name and a number when
 *  another folder already had it. */
function carries(folder: string, space: string): boolean {
  const have = nameKey(folder);
  const want = nameKey(space.trim());
  return have === want || (have.startsWith(`${want} `) && /^(\d+|\(space\))$/u.test(have.slice(want.length + 1)));
}

function ownSpace(page: Pick<Page, "space">): string | null {
  const space = page.space?.trim();
  return space && space !== "uncategorized" ? space : null;
}

/** The Space folders the pages themselves show: with Space folders on, each
 *  page with a Space sits in that Space's folder, at the top for a personal
 *  Space and inside the team's folder for a team's. `on` is the daemon's
 *  own setting; off, no folder is a Space's. */
export function spaceLayout(
  pages: readonly Pick<Page, "space" | "storage_path" | "folder_path" | "status">[],
  on: boolean,
  teamSpaces: ReadonlySet<string> = new Set(),
): SpaceLayout {
  if (!on) return NO_SPACE_LAYOUT;
  const spaceFolders = new Set<string>();
  const teamFolders = new Set<string>();
  for (const page of pages) {
    const space = ownSpace(page);
    if (!space) continue;
    const folders = parts(pageFolderPath(page as Page));
    if (teamSpaces.has(space)) {
      if (folders.length >= 2 && carries(folders[1], space)) {
        spaceFolders.add(`${folders[0]}/${folders[1]}`);
        teamFolders.add(folders[0]);
      }
    } else if (folders.length >= 1 && carries(folders[0], space)) {
      spaceFolders.add(folders[0]);
    }
  }
  return { spaceFolders: [...spaceFolders], teamFolders: [...teamFolders] };
}

function under(folders: readonly string[], holder: string): string[] | null {
  const want = parts(holder);
  if (folders.length < want.length || want.some((name, index) => nameKey(name) !== nameKey(folders[index]))) return null;
  return folders.slice(want.length);
}

type Placed = { readonly spaceFolder: string | null; readonly inTeam: boolean; readonly inside: readonly string[] };

/** Where a folder path sits among the Spaces' folders: the deepest Space
 *  folder that holds it and the folders inside that one. */
function placed(path: string | null, layout: SpaceLayout): Placed {
  const folders = parts(path);
  let best: { folder: string; rest: string[] } | null = null;
  for (const folder of layout.spaceFolders) {
    const rest = under(folders, folder);
    if (rest && (!best || folder.length > best.folder.length)) best = { folder, rest };
  }
  if (best) return { spaceFolder: best.folder, inTeam: best.folder.includes("/"), inside: best.rest };
  for (const team of layout.teamFolders) {
    const rest = under(folders, team);
    if (rest) return { spaceFolder: null, inTeam: true, inside: rest };
  }
  return { spaceFolder: null, inTeam: false, inside: folders };
}

/** The section a folder path belongs to: Rules or Skills when a folder of
 *  that name is on the path, at any depth, inside the Space's own folder;
 *  Docs otherwise. `inSpace` says the page is in a Space. A page in no Space
 *  whose folder is both a Space's and named Rules (a Space that happens to
 *  be called Rules) still reads as a rules page, as the daemon reads it. */
export function folderSection(path: string | null, layout: SpaceLayout = NO_SPACE_LAYOUT, inSpace = false): PageSection {
  const where = placed(path, layout);
  const plain = !inSpace && where.spaceFolder !== null && !where.inTeam
    && (named(where.spaceFolder, RULES_FOLDER_NAMES) || named(where.spaceFolder, SKILLS_FOLDER_NAMES));
  const inside = plain ? parts(path) : where.inside;
  if (inside.some((folder) => named(folder, RULES_FOLDER_NAMES))) return "rules";
  if (inside.some((folder) => named(folder, SKILLS_FOLDER_NAMES))) return "skills";
  return "docs";
}

/** Which section lists `page`. A "How I work" page is a rules page wherever
 *  its file is, and so is the page that keeps the rules the person asked to
 *  be saved. Every page is in exactly one section. */
export function pageSection(
  page: Pick<Page, "content" | "title" | "space" | "storage_path" | "folder_path" | "status">,
  layout: SpaceLayout = NO_SPACE_LAYOUT,
): PageSection {
  if (isHowIWorkPage(page) || isSavedRulesPage(page)) return "rules";
  return folderSection(pageFolderPath(page as Page), layout, ownSpace(page) !== null);
}

/** The folder a new page of `section` goes in: one carrying the section's
 *  name that the person keeps outside every Space's and team's folder,
 *  nearest the top first. Null when there is none yet. A folder inside a
 *  Space gives rules to that Space only, so it is never picked here. */
export function sectionFolderPath(
  section: Exclude<PageSection, "docs">,
  folderPaths: readonly string[],
  layout: SpaceLayout = NO_SPACE_LAYOUT,
): string | null {
  const names = section === "rules" ? RULES_FOLDER_NAMES : SKILLS_FOLDER_NAMES;
  const matches = folderPaths.filter((path) => {
    if (!named(path.split("/").pop() ?? "", names)) return false;
    const where = placed(path, layout);
    return where.spaceFolder === null && !where.inTeam && folderSection(path, layout) === section;
  });
  return matches.sort((left, right) => left.split("/").length - right.split("/").length || left.localeCompare(right))[0] ?? null;
}

/** Whether a folder of this name at the top of the pages folder is already a
 *  Space's own, so it cannot become the section's folder. */
export function topFolderIsASpaces(name: string, layout: SpaceLayout): boolean {
  return layout.spaceFolders.some((folder) => !folder.includes("/") && nameKey(folder) === nameKey(name))
    || layout.teamFolders.some((folder) => nameKey(folder) === nameKey(name));
}
