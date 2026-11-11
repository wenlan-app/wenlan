// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

const BANNER = "This page is archived. Restore it to edit.";

/** A page archived earlier whose title an active page has taken since. */
function fixtureWithArchivedTwin() {
  const fixture = createSpacesNavigationFixture();
  const recap = fixture.pages.find(note => note.id === "page-keyboard")!;
  return {
    ...fixture,
    pages: [...fixture.pages, {
      ...recap, id: "page-recap-2025", version: 3, status: "archived",
      content: "# Weekly fixture recap\n\nLast year's recap, put away.",
      last_modified: "2025-12-01T12:00:00Z",
    }],
  };
}

async function openForWriting(page: Page, title: string): Promise<void> {
  await openWikiNote(page, title);
  await expect(page.getByRole("textbox", { name: "Page editor", exact: true })).toBeVisible();
}

async function confirm(page: Page, action: string): Promise<void> {
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: action, exact: true }).click();
  await expect(dialog).toHaveCount(0);
}

async function stored(page: Page, id: string) {
  return page.evaluate(async pageId => window.__TAURI_INTERNALS__!.invoke("get_page", { id: pageId }), id) as Promise<{ status: string; title: string; version: number; content: string }>;
}

test("archive keeps the tab read only; Archived pages restores, through a title conflict", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [], fixture: fixtureWithArchivedTwin() });
  await page.goto("/");

  // Archive from the page's own menu: the tab stays, read only, with Restore.
  await openForWriting(page, "History semantics");
  await page.getByRole("button", { name: "Page actions" }).click();
  await page.getByRole("menuitem", { name: "Archive page" }).click();
  await expect(page.getByRole("alertdialog")).toContainText("Archive this page? You can restore it from Archived pages.");
  await confirm(page, "Archive page");
  await expect(page.getByText(BANNER)).toBeVisible();
  await expect(page.getByRole("heading", { level: 1, name: "History semantics" })).toBeVisible();
  await expect(page.locator('[data-note-tab-group="primary"] [role="tab"]', { hasText: "History semantics" })).toHaveCount(1);
  expect(await stored(page, "page-history")).toMatchObject({ status: "archived", version: 2 });
  await page.getByRole("button", { name: "Page actions" }).click();
  await expect(page.getByRole("button", { name: "Edit page" })).toHaveCount(0);
  await expect(page.getByRole("menuitem", { name: "Archive page" })).toHaveCount(0);
  await page.keyboard.press("Escape");

  // The page left the folder tree, which has no browse menu (UX-FIND-4).
  const directory = page.locator(".wiki-workspace-directory");
  await expect(directory.locator(".notes-inventory-page").getByText("History semantics", { exact: true })).toHaveCount(0);
  await expect(directory.getByRole("combobox")).toHaveCount(0);

  // Search finds archived pages, in their own group.
  await page.locator(".notes-workspace-sidebar").getByRole("button", { name: "Search", exact: true }).click();
  const search = page.getByRole("dialog", { name: "Search", exact: true });
  await search.getByRole("textbox").fill("weekly recap");
  const found = search.getByRole("region", { name: "Archived pages" });
  await expect(found.getByRole("button")).toHaveText([/^Weekly fixture recap/]);
  await found.getByRole("button").click();
  await expect(search).toHaveCount(0);
  await expect(page.getByText(BANNER)).toBeVisible();

  // Restoring under a title another active page took is refused by its typed code.
  await page.getByRole("button", { name: "Restore page" }).click();
  const dialog = page.getByRole("dialog", { name: "Restore page" });
  await dialog.getByRole("button", { name: "Restore page" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Another page already has this title. Choose a different title.");
  expect(await stored(page, "page-recap-2025")).toMatchObject({ status: "archived", version: 3 });
  await dialog.getByRole("textbox", { name: "Page title" }).fill("Weekly fixture recap 2025");
  await dialog.getByRole("button", { name: "Restore page" }).click();
  await expect(dialog).toHaveCount(0);
  expect(await stored(page, "page-recap-2025")).toMatchObject({
    status: "active", title: "Weekly fixture recap 2025", version: 4,
    content: "# Weekly fixture recap\n\nLast year's recap, put away.",
  });

  // The home's Archived pages list holds what is left, and restores from it.
  await openPrimaryDestination(page, "Home");
  await page.locator(".wiki-archive-entry").click();
  const archived = directory.getByRole("region", { name: "Archived pages" });
  await expect(archived.locator(".wiki-archived-open")).toHaveText([/^History semantics/]);
  await archived.getByRole("button", { name: /^Restore History semantics/ }).click();
  await page.getByRole("dialog", { name: "Restore page" }).getByRole("button", { name: "Restore page" }).click();
  expect(await stored(page, "page-history")).toMatchObject({ status: "active", title: "History semantics", version: 3 });
  await expect(archived).toContainText("No archived pages.");
  // Back to folders: the restored page is in the tree again.
  await directory.getByRole("button", { name: "Back to folders" }).click();
  await expect(directory.locator(".notes-inventory-page").getByText("History semantics", { exact: true })).toHaveCount(1);

  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("Archive from the folder tree, without deleting anything", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  await openForWriting(page, "Fixture architecture");

  const directory = page.locator(".wiki-workspace-directory");
  // Folder tree: right-click a page that is not on screen.
  await directory.locator(".notes-inventory-page").getByText("Weekly fixture recap", { exact: true }).click({ button: "right" });
  const treeMenu = page.getByRole("menu");
  await expect(treeMenu.getByRole("menuitem")).toHaveText(["Open", "Archive page", "Delete page"]);
  await treeMenu.getByRole("menuitem", { name: "Archive page" }).click();
  await confirm(page, "Archive page");
  await expect(directory.locator(".notes-inventory-page").getByText("Weekly fixture recap", { exact: true })).toHaveCount(0);
  expect(await stored(page, "page-keyboard")).toMatchObject({ status: "archived" });

  // The open page archives through its editor.
  await directory.locator(".notes-inventory-page").getByText("Fixture architecture", { exact: true }).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Archive page" }).click();
  await confirm(page, "Archive page");
  await expect(page.getByText(BANNER)).toBeVisible();
  await expect(directory.locator(".notes-inventory-page").getByText("Fixture architecture", { exact: true })).toHaveCount(0);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
