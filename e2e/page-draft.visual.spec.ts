// SPDX-License-Identifier: AGPL-3.0-only
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

const evidenceDir = path.join(
  process.cwd(),
  ".omo/evidence/page-draft-lifecycle",
);

async function settle(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await document.fonts.ready;
  });
  await expect(page.locator("main")).toBeVisible();
}

async function openPrimaryDestination(
  page: Page,
  destination: "Wiki" | "Spaces",
): Promise<void> {
  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  const sidebar = page.locator('aside[aria-label="Primary navigation"]');
  if (await sidebar.getAttribute("aria-hidden") === "true") {
    await page.getByTitle("Show sidebar").click();
  }
  await navigation.getByRole("button", { name: destination, exact: true }).click();
  if (destination === "Wiki") await expect(page.locator(".wiki-workspace")).toBeVisible();
  else await expect(page.getByRole("heading", { level: 1, name: destination })).toBeAttached();
}

async function controlMetrics(page: Page, name: "New page" | "New") {
  return page.getByRole("button", { name, exact: true }).evaluate((element) => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return {
      borderColor: style.borderColor,
      borderRadius: style.borderRadius,
      fontSize: style.fontSize,
      height: rect.height,
      lineHeight: style.lineHeight,
      padding: style.padding,
    };
  });
}

test("captures direct Page authoring from the tab action and Space detail", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await mkdir(evidenceDir, { recursive: true });
  const runtime = await installTauriMock(page, {
    locale: "en",
    localStorage: { "wenlan-theme": "dark", "wenlan-spaces-view-mode": "rows" },
    rawActions: [],
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");

  await openPrimaryDestination(page, "Wiki");
  await settle(page);
  const newPageMetrics = await controlMetrics(page, "New page");
  await page.screenshot({
    path: path.join(evidenceDir, "wiki-1280x900.png"),
    fullPage: false,
  });

  await page.locator(".note-tab-create").click();
  await expect(page.getByRole("textbox", { name: "Title", exact: true })).toBeFocused();
  const wikiDraftForm = page.getByRole("region", { name: "New page", exact: true });
  await expect(wikiDraftForm.getByRole("combobox")).toHaveCount(0);
  await expect(wikiDraftForm.getByRole("button", { name: "Publish", exact: true })).toHaveCount(0);
  await settle(page);
  await page.screenshot({
    path: path.join(evidenceDir, "draft-wiki-1280x900.png"),
    fullPage: false,
  });
  await page.getByRole("button", { name: "Back", exact: true }).click();
  expect(runtime.calls().filter((call) => call.command === "create_page_draft")).toEqual([]);

  await openPrimaryDestination(page, "Spaces");
  await settle(page);
  const newSpaceMetrics = await controlMetrics(page, "New");
  expect(newPageMetrics.height).toBeGreaterThanOrEqual(32);
  expect(newSpaceMetrics.height).toBeGreaterThanOrEqual(32);
  await page.screenshot({
    path: path.join(evidenceDir, "spaces-1280x900.png"),
    fullPage: false,
  });

  await page
    .getByTestId("space-row-space-wenlan")
    .getByRole("button", { name: "Wenlan", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await settle(page);
  await page.screenshot({
    path: path.join(evidenceDir, "space-detail-1280x900.png"),
    fullPage: false,
  });

  await page.getByRole("button", { name: "New page", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Title", exact: true })).toBeVisible();
  const spaceDraftForm = page.getByRole("region", { name: "New page", exact: true });
  await expect(spaceDraftForm.getByRole("combobox")).toHaveCount(0);
  await expect(spaceDraftForm.getByRole("button", { name: "Publish", exact: true })).toHaveCount(0);
  await page.getByRole("textbox", { name: "Title", exact: true }).fill("Space-scoped draft");
  await expect.poll(() => runtime.calls().find((call) => call.command === "create_page_draft")?.args)
    .toMatchObject({ space: "Wenlan" });
  await settle(page);
  await page.screenshot({
    path: path.join(evidenceDir, "draft-space-1280x900.png"),
    fullPage: false,
  });

  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("keeps the editor axis stable on wide screens and title conflicts readable on mobile", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await mkdir(evidenceDir, { recursive: true });
  const runtime = await installTauriMock(page, {
    locale: "en",
    localStorage: { "wenlan-theme": "dark", "wenlan-spaces-view-mode": "rows" },
    rawActions: [],
  });
  await page.setViewportSize({ width: 1920, height: 900 });
  await page.goto("/");
  await openPrimaryDestination(page, "Spaces");
  await page
    .getByTestId("space-row-space-wenlan")
    .getByRole("button", { name: "Wenlan", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await page.getByRole("button", { name: "New page", exact: true }).click();
  await expect(page.getByRole("region", { name: "New page", exact: true })).toBeVisible();
  const draftContentEdge = await page.locator(".page-draft-editor").evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.x + Number.parseFloat(getComputedStyle(element).paddingLeft);
  });
  const editorMetrics = await page.locator(".page-draft-editor-axis").evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return { width: rect.width, x: rect.x };
  });
  expect(Math.abs(editorMetrics.x - draftContentEdge)).toBeLessThanOrEqual(0.5);
  expect(editorMetrics.width).toBe(730);

  await page.getByRole("textbox", { name: "Title", exact: true }).fill("Fixture architecture");
  await page.getByRole("textbox", { name: "Content", exact: true }).fill(
    "Conflict layout verification body.",
  );
  await expect.poll(() => runtime.calls().find((call) => call.command === "create_page_draft")?.args)
    .toMatchObject({ space: "Wenlan" });
  await page.locator(".note-tab-create").click();
  await expect(page.getByRole("alert")).toContainText(
    "A page with this title already exists.",
  );

  await page.setViewportSize({ width: 375, height: 812 });
  const alert = page.getByRole("alert");
  await expect(alert.getByRole("button", { name: "Open existing", exact: true })).toHaveCount(0);
  await expect(alert.getByRole("button", { name: "Rename page", exact: true })).toBeVisible();
  await expect(alert).toHaveCSS("flex-direction", "column");
  const overflow = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
  await settle(page);
  await page.screenshot({
    path: path.join(evidenceDir, "draft-title-conflict-375x812.png"),
    fullPage: false,
  });

  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
