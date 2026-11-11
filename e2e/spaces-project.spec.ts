// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpaceProjectFixture } from "./fixtures/spaceProject";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

const items = ["Sign the Windows builds", "Record this week's update video", "Let a new person add anything from one place", "Keep only conclusions from imported chats", "Tidy the welcome page"];

function withBrief() {
  const fixture = createSpaceProjectFixture();
  return { ...fixture, spaceBriefs: { Wenlan: { summary: "The trust fixes are all merged.", handed_off_at: Math.floor(Date.now() / 1000) - 3 * 3600, active: items.map((line, i) => ({ id: `item-${i}`, line, raw: false })) } } };
}

test("UX-SPACE-1 a Space opens on its Brief above its pages, with no tabs", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", rawActions: [], fixture: withBrief(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  const brief = page.getByRole("region", { name: "Where this stands", exact: true });
  await expect(brief).toContainText("The trust fixes are all merged.");
  await expect(brief).toContainText("Last handoff");
  await expect(brief.getByRole("listitem")).toHaveCount(3);
  await brief.getByRole("button", { name: "2 more", exact: true }).click();
  await expect(brief.getByRole("listitem")).toHaveCount(5);
  await brief.getByRole("button", { name: "Show less", exact: true }).click();
  await expect(brief.getByRole("listitem")).toHaveCount(3);
  await expect(page.getByRole("tab")).toHaveCount(0);
  await expect(page.locator(".space-dossier-page-list")).toBeVisible();
  expect(controller.calls().filter(call => call.command === "get_space_brief").at(-1)?.args).toEqual({ name: "Wenlan" });
  expect(controller.calls().filter(call => ["list_memories_cmd", "list_entities_cmd"].includes(call.command) && JSON.stringify(call.args).includes('"Wenlan"'))).toEqual([]);
  await page.getByRole("button", { name: "Actions for Wenlan" }).press("ArrowDown");
  await expect(page.getByRole("menuitem", { name: "Review page changes" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Actions for Wenlan" })).toBeFocused();
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("UX-SPACE-1 a Space without a Brief is its pages alone, and new pages inherit the Space", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", rawActions: [], fixture: withBrief(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-reading").getByRole("button", { name: "讀書會", exact: true }).click();
  await expect(page.getByRole("heading", { name: "讀書會", level: 1 })).toBeVisible();
  await expect.poll(() => controller.calls().some(call => call.command === "get_space_brief")).toBe(true);
  await expect(page.getByTestId("space-brief")).toHaveCount(0);
  await page.getByRole("button", { name: "New page", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Content", exact: true })).toBeVisible();
  // New notes inherit their Space without an extra creation form.
  await expect(page.getByRole("region", { name: "New page", exact: true }).getByRole("combobox")).toHaveCount(0);
  await page.getByRole("textbox", { name: "Title", exact: true }).fill("Reading observation");
  await page.getByRole("textbox", { name: "Content", exact: true }).fill("A page created inside the reading club.");
  await expect.poll(() => controller.calls().find(call => call.command === "create_page_draft")?.args).toMatchObject({ space: "讀書會" });
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("UX-SPACE-1 the Brief and pages fit a narrow window", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 375, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], fixture: withBrief(), localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page.getByTestId("space-row-project-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  const overflow = () => page.evaluate(async () => { await document.fonts.ready; return document.documentElement.scrollWidth - innerWidth; });
  await expect(page.getByTestId("space-brief")).toBeVisible();
  expect(await overflow()).toBeLessThanOrEqual(1);
  await page.getByRole("button", { name: "2 more", exact: true }).click();
  expect(await overflow()).toBeLessThanOrEqual(1);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
