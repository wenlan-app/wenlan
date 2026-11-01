// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Page } from "@playwright/test";

/**
 * Home holds what is in motion and has no directory; every page lives in the
 * Wiki. A journey that browses pages from Home goes to the Wiki first.
 */
export async function leaveHomeForWiki(page: Page) {
  const workspace = page.locator(".wiki-workspace");
  await expect(workspace.first()).toBeAttached();
  if (await page.locator('.wiki-workspace[data-directory-hidden="true"]').count() === 0) return;
  const wiki = page.locator('.notes-rail-nav [aria-label="Wiki"]');
  // A narrow window, or a hidden sidebar, keeps the rail closed until asked.
  const show = page.locator('[data-sidebar-toggle][title="Show sidebar"]');
  if (await show.count() && !(await wiki.isVisible())) await show.click();
  await wiki.click();
  await expect(page.locator('.wiki-workspace[data-directory-hidden="true"]')).toHaveCount(0);
}

/** Switch the optional pages panel to folders, including narrow screens. */
export async function showWikiFolders(page: Page) {
  await leaveHomeForWiki(page);
  const directory = page.locator(".wiki-workspace-directory");
  await expect(directory).toBeAttached();
  if (!(await directory.isVisible())) await page.locator(".wiki-workspace-folders-toggle").click();
  await expect(directory).toBeVisible();
  await directory.getByRole("combobox").selectOption("folders");
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
