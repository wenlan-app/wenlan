// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpaceProjectFixture } from "./fixtures/spaceProject";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

test("reads only current Space sources and opens pages through the shared editor", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", rawActions: [], fixture: createSpaceProjectFixture(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".space-dossier-archive,.space-dossier-entities,.space-dossier-review")).toHaveCount(0);
  expect(controller.calls().filter(call => ["list_memories_cmd", "list_entities_cmd"].includes(call.command) && JSON.stringify(call.args).includes('"Wenlan"'))).toEqual([]);
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByText("Writing experience observations", { exact: true }).click();
  await expect(page.getByText("Start with the page itself. Keep references close when the reader needs them.")).toBeVisible();
  await expect(page.getByRole("complementary", { name: "Writing experience observations" })).toBeVisible();
  expect(controller.calls().filter(call => call.command === "get_chunks").at(-1)?.args).toEqual({ source: "file", sourceId: "design.md", space: "Wenlan" });
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await expect(page.getByRole("complementary", { name: "Writing experience observations" })).toHaveCount(0);
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-reading").getByRole("button", { name: "讀書會", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.getByText("Writing experience observations")).toHaveCount(0);
  await page.getByText("讀書會摘錄", { exact: true }).click();
  await expect(page.getByText("這份文件只屬於讀書會，用來驗證來源讀取不會跨越空間。")).toBeVisible();
  expect(controller.calls().filter(call => call.command === "get_chunks").at(-1)?.args).toEqual({ source: "file", sourceId: "design.md", space: "讀書會" });
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "New page", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Content", exact: true })).toBeVisible();
  // New notes inherit their Space without an extra creation form.
  await expect(page.getByRole("region", { name: "New page", exact: true }).getByRole("combobox")).toHaveCount(0);
  await page.getByRole("textbox", { name: "Title", exact: true }).fill("Reading observation");
  await page.getByRole("textbox", { name: "Content", exact: true }).fill("A page created inside the reading club.");
  await expect.poll(() => controller.calls().find(call => call.command === "create_page_draft")?.args).toMatchObject({ space: "讀書會" });
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("source preview Escape returns focus to the activating row and tab switch drops the preview", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [], fixture: createSpaceProjectFixture(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  const row = page.getByRole("button", { name: /Writing experience observations/ });
  await row.focus();
  await expect(row).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("complementary", { name: "Writing experience observations" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("complementary", { name: "Writing experience observations" })).toHaveCount(0);
  await expect(row).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("complementary", { name: "Writing experience observations" })).toBeVisible();
  await page.getByRole("tab", { name: "Pages" }).click();
  await expect(page.getByRole("complementary", { name: "Writing experience observations" })).toHaveCount(0);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("Space source failures retry and remain distinct from an empty document list", async ({ page }) => {
  const controller = await installTauriMock(page, { locale: "en", rawActions: [], fixture: createSpaceProjectFixture(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/"); await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  controller.failNext("list_indexed_files", "source read unavailable");
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.getByRole("alert")).toContainText("Could not load sources for this space.");
  await expect(page.getByText("No sources in this space yet.")).toHaveCount(0);
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByText("Writing experience observations")).toBeVisible();
  await page.getByRole("button", { name: "Actions for Wenlan" }).press("ArrowDown");
  await expect(page.getByRole("menuitem", { name: "Review page changes" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Actions for Wenlan" })).toBeFocused();
});

test("project Pages, Sources and the source preview fit a narrow window", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 375, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], fixture: createSpaceProjectFixture(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  const overflow = () => page.evaluate(async () => { await document.fonts.ready; return document.documentElement.scrollWidth - innerWidth; });
  await expect(page.getByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
  expect(await overflow()).toBeLessThanOrEqual(1);
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.getByText("Writing experience observations", { exact: true })).toBeVisible();
  expect(await overflow()).toBeLessThanOrEqual(1);
  await page.getByText("Writing experience observations", { exact: true }).click();
  await expect(page.getByText("Start with the page itself. Keep references close when the reader needs them.")).toBeVisible();
  expect(await overflow()).toBeLessThanOrEqual(1);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
