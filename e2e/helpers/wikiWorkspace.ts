// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Locator, type Page } from "@playwright/test";
import { resources } from "../../src/i18n/resources";

const SECTION_LABELS = {
  docs: Object.values(resources).map(({ translation: t }) => t.sidebar.pages),
  skills: Object.values(resources).map(({ translation: t }) => t.sidebar.skills),
  rules: Object.values(resources).map(({ translation: t }) => t.sidebar.rules),
} as const;

export type SidebarSection = keyof typeof SECTION_LABELS;

function labelPattern(labels: readonly string[]): RegExp {
  return new RegExp(`^(${[...new Set(labels)].join("|")})$`, "u");
}

/** The list page of a section, once its pages and folders have loaded. */
export function sectionList(page: Page, section?: SidebarSection): Locator {
  return page.locator(`.section-list[aria-busy="false"]${section ? `[data-section="${section}"]` : ""}`);
}

/**
 * The left side lists no pages (UX-FIND-4): Docs, Skills and Rules each open
 * a list page in the main area. This opens one from the sidebar, in any app
 * language, and waits for the list.
 */
export async function openSection(page: Page, section: SidebarSection = "docs") {
  await expect(page.locator(".wiki-workspace, .notes-rail-nav").first()).toBeAttached();
  const row = page.locator(".notes-rail-nav").getByRole("button", { name: labelPattern(SECTION_LABELS[section]) });
  // A narrow window, or a hidden sidebar, keeps the rail closed until asked.
  const toggle = page.locator("[data-sidebar-toggle]");
  await expect.poll(async () => await row.isVisible() || await toggle.first().isVisible()).toBe(true);
  if (!(await row.isVisible())) await toggle.first().click();
  await row.click();
  const list = sectionList(page, section);
  await expect(list).toBeVisible();
  return list;
}

/** A journey that browses pages from Home goes to the Docs list first. */
export async function leaveHomeForWiki(page: Page) {
  await openSection(page, "docs");
}

/** Show the Docs list: folders as cards, then the pages in no folder. */
export async function showWikiFolders(page: Page) {
  return openSection(page, "docs");
}

/** Open the folder at `path` ("Work/Projects") from the top of Docs, one card at a time. */
export async function openDocsFolder(page: Page, path: string) {
  await openSection(page, "docs");
  let current = "";
  for (const name of path.split("/")) {
    current = current ? `${current}/${name}` : name;
    await sectionList(page, "docs").locator(`.section-list-folders > li[data-folder-path="${current}"] > button`).click();
    await expect(sectionList(page, "docs").locator(".section-list-back")).toBeVisible();
    await expect(sectionList(page, "docs").getByRole("heading", { level: 1, name, exact: true })).toBeVisible();
  }
  return sectionList(page, "docs");
}

async function findListedNote(page: Page, title: string): Promise<Locator | null> {
  const list = sectionList(page);
  await expect(list).toBeVisible();
  const note = list.locator(".section-list-page-title").getByText(title, { exact: true });
  if (await note.count()) return note.first();
  const paths = await list.locator(".section-list-folders > li[data-folder-path]").evaluateAll(
    (items) => items.map((item) => item.getAttribute("data-folder-path") ?? ""),
  );
  for (const path of paths) {
    await list.locator(`.section-list-folders > li[data-folder-path="${path}"] > button`).click();
    await expect(page.locator(`.section-list-folders > li[data-folder-path="${path}"]`)).toHaveCount(0);
    const found = await findListedNote(page, title);
    if (found) return found;
    await sectionList(page).locator(".section-list-back").click();
    await expect(sectionList(page).locator(`.section-list-folders > li[data-folder-path="${path}"]`)).toBeVisible();
  }
  return null;
}

/**
 * Find a page's row, looking through Docs folders as a person would, then
 * Rules and Skills. The list that holds it stays on screen; null when no
 * list has it.
 */
export async function findWikiNote(page: Page, title: string): Promise<Locator | null> {
  for (const section of ["docs", "rules", "skills"] as const) {
    await openSection(page, section);
    const found = await findListedNote(page, title);
    if (found) return found;
  }
  return null;
}

/** Open a page from its section's list. */
export async function openWikiNote(page: Page, title: string) {
  const note = await findWikiNote(page, title);
  if (!note) throw new Error(`No page titled "${title}" is listed under Docs, Rules or Skills`);
  await note.click();
}

/** Open the page titled `title` from the Docs folder at `folder` (null: the top of Docs). */
export async function openDocsPage(page: Page, folder: string | null, title: string) {
  const list = folder === null ? await openSection(page, "docs") : await openDocsFolder(page, folder);
  await list.locator(".section-list-page-title").getByText(title, { exact: true }).click();
}
