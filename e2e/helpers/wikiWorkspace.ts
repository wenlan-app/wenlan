// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Page } from "@playwright/test";

/**
 * With a labelled sidebar, every page is listed in it, below the
 * destinations, on any screen. Otherwise Home has no directory, and a journey
 * that browses pages from Home goes to the Wiki first.
 */
export async function leaveHomeForWiki(page: Page) {
  const workspace = page.locator(".wiki-workspace");
  await expect(workspace.first()).toBeAttached();
  const sidebarWiki = page.locator('.wiki-sidebar-directory .notes-wiki-heading');
  if (await sidebarWiki.isVisible()) {
    // Every page is listed in the sidebar; the journey still browses from the Wiki.
    if (await sidebarWiki.getAttribute("aria-current") !== "page") await sidebarWiki.click();
    return;
  }
  if (await page.locator('.wiki-workspace[data-directory-hidden="true"]').count() === 0) return;
  const wiki = page.locator('.notes-rail-nav [aria-label="Wiki"]');
  // A narrow window, or a hidden sidebar, keeps the rail closed until asked.
  await expect(wiki).toBeAttached();
  if (!(await wiki.isVisible())) await page.locator("[data-sidebar-toggle]").click();
  await wiki.click();
  await expect(page.locator(".wiki-workspace-directory")).toBeAttached();
  if (await page.locator(".wiki-sidebar-directory").count()) return;
  await expect(page.locator('.wiki-workspace[data-directory-hidden="true"]')).toHaveCount(0);
}

/** Show the folder tree, including on narrow screens. */
export async function showWikiFolders(page: Page) {
  await leaveHomeForWiki(page);
  const directory = page.locator(".wiki-workspace-directory");
  await expect(directory).toBeAttached();
  if (!(await directory.isVisible())) await page.locator(".wiki-workspace-folders-toggle").click();
  await expect(directory).toBeVisible();
  await expect(directory.locator(".notes-list-panel")).toBeVisible();
  return directory;
}

/** Open a page through the optional folder view. */
export async function openWikiNote(page: Page, title: string) {
  const directory = await showWikiFolders(page);
  await expect(directory.locator(".notes-inventory-page, .notes-inventory-scope").first()).toBeVisible();
  const note = directory.locator(".notes-inventory-page").getByText(title, { exact: true });
  for (let count = 0; count < 40 && !(await note.isVisible()); count++) {
    const expand = directory.locator('.notes-inventory-disclosure[aria-expanded="false"]').first();
    if (!(await expand.count())) break;
    await expand.click();
  }
  await expect(note).toBeVisible();
  await note.click();
}
