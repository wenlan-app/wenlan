// SPDX-License-Identifier: AGPL-3.0-only
import { returnToPageReading } from "./helpers/pageReading";
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { openWikiNote } from "./helpers/wikiWorkspace";

async function openFixturePage(page: Page): Promise<void> {
  await page.goto("/");
  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  await navigation.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();
  await openWikiNote(page, "Fixture architecture");
  await expect(page.getByRole("heading", { level: 1, name: "Fixture architecture" })).toBeVisible();
  await returnToPageReading(page);
}

async function requestDelete(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Page actions" }).click();
  await page.getByRole("menuitem", { name: "Delete page" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete page", exact: true }).click();
}

test("deletes a Wiki Page and removes it from the inventory", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await openFixturePage(page);

  await requestDelete(page);

  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();
  await expect(page.locator(".wiki-workspace-directory").getByText("Fixture architecture", { exact: true })).toHaveCount(0);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("keeps a failed Wiki Page deletion visible and retryable", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", rawActions: [] });
  controller.failNext("delete_page", "daemon offline");
  await openFixturePage(page);

  await requestDelete(page);

  await expect(page.getByRole("alert")).toHaveText("Could not delete this page. Try again.");
  await expect(page.getByRole("heading", { level: 1, name: "Fixture architecture" })).toBeVisible();

  await requestDelete(page);
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();
  await expect(page.locator(".wiki-workspace-directory").getByText("Fixture architecture", { exact: true })).toHaveCount(0);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("deleting a Page from the right group removes only that tab", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await installTauriMock(page, {
    fixture: createReviewDecisionFixture("wiki-folders"),
    locale: "en",
    rawActions: [],
  });
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  await openWikiNote(page, "Why fixtures stay deterministic");

  const primary = page.locator('[data-note-group-id="primary"]');
  const secondary = page.locator('[data-note-group-id="secondary"]');
  const rightPage = primary.getByRole("tab", { name: "Why fixtures stay deterministic", exact: true });
  await rightPage.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to right group", exact: true }).click();
  await expect(secondary.getByRole("tab", { name: "Why fixtures stay deterministic", exact: true })).toBeVisible();
  await expect(primary.getByRole("tab", { name: "Fixture architecture", exact: true })).toBeVisible();

  const editor = secondary.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await editor.press("Escape");
  await expect(editor).toHaveCount(0);
  await secondary.getByRole("button", { name: "Page actions" }).click();
  await secondary.getByRole("menuitem", { name: "Delete page" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete page", exact: true }).click();

  await expect(secondary).toHaveCount(0);
  await expect(primary.getByRole("tab", { name: "Fixture architecture", exact: true })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Why fixtures stay deterministic", exact: true })).toHaveCount(0);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
