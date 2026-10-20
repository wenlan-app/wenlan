// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openSpaceEntity } from "./helpers/spaceEntity";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

test("Wiki -> Spaces -> Space -> Page -> back and Space -> global Topics -> Topic -> back", async ({ page }) => {
  // Keep the selected Wiki note long enough to exercise reading-pane scrolling.
  await page.setViewportSize({ width: 1280, height: 500 });
  // Given a clean fixture and browser error capture.
  const browserErrors = collectBrowserErrors(page);
  // Rows lens: this journey asserts the Wiki page scrolls and resets on
  // navigation, and the default Cards grid fits the fixture inside the viewport.
  const fixture = createSpacesNavigationFixture();
  const longNote = fixture.pages.find((item) => item.id === "page-architecture")!;
  longNote.content = "# Fixture architecture\n\n" + Array.from({ length: 70 }, (_, index) => `Paragraph ${index}: a long page makes the current Wiki reading pane scrollable.\n\n`).join("");
  await installTauriMock(page, { fixture, locale: "en", localStorage: { "wenlan-wiki-view-mode": "rows", "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");

  // When the two primary hierarchy journeys are driven through the rendered shell.
  const primaryNavigation = page.getByRole("navigation", { name: "Primary navigation" });
  await openPrimaryDestination(page, "Wiki");
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);
  const more = primaryNavigation.getByRole("button", { name: "More", exact: true });
  const wiki = primaryNavigation.getByRole("button", { name: "Wiki", exact: true });
  await expect(wiki).toHaveAttribute("aria-current", "page");
  await expect(more).not.toHaveAttribute("aria-current");
  await more.click();
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);
  await expect(wiki).toHaveAttribute("aria-current", "page");
  await expect(primaryNavigation.getByRole("button", { name: "Home", exact: true })).toHaveCount(0);
  await expect(more).not.toHaveAttribute("aria-current");
  await page.keyboard.press("Escape");
  await expect(wiki).toHaveAttribute("aria-current", "page");
  await expect(more).not.toHaveAttribute("aria-current");

  await primaryNavigation.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();
  await expect(primaryNavigation.getByRole("button", { name: "Wiki" })).toHaveAttribute("aria-current", "page");
  await openWikiNote(page, "Independent research");
  await expect(page.getByRole("heading", { level: 1, name: "Independent research" })).toBeVisible();
  await expect(primaryNavigation.getByRole("button", { name: "Wiki" })).toHaveAttribute("aria-current", "page");
  await page.getByRole("group", { name: "History navigation" }).getByRole("button", { name: "Back", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();

  await openWikiNote(page, "Fixture architecture");
  const wikiContent = page.locator(".wiki-workspace-content");
  await wikiContent.evaluate((node) => { node.scrollTop = node.scrollHeight; });
  await expect.poll(() => wikiContent.evaluate((node) => node.scrollTop)).toBeGreaterThan(0);

  await primaryNavigation.getByRole("button", { name: "Spaces", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
  await expect.poll(() => page.locator("main").evaluate((node) => node.scrollTop)).toBe(0);
  await expect(page.getByRole("navigation", { name: "Recent spaces" })).toHaveCount(0);
  const wenlanRow = page.getByTestId("space-row-space-wenlan");
  await wenlanRow.getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Recent spaces" })).toHaveCount(0);
  const spaces = primaryNavigation.getByRole("button", { name: "Spaces", exact: true });
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);
  await expect(spaces).toHaveAttribute("aria-current", "page");
  await expect(spaces).not.toHaveAttribute("aria-pressed");
  await wenlanRow.getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);
  await expect(spaces).toHaveAttribute("aria-current", "page");

  await page.getByRole("button", { name: /Fixture architecture/ }).first().click();
  await expect(page.getByRole("heading", { level: 1, name: "Fixture architecture" })).toBeVisible();
  await page.getByRole("group", { name: "History navigation" }).getByRole("button", { name: "Back", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();

  await openSpaceEntity(page, "Ada Lovelace");
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();
  await page.keyboard.press("Escape");

  // Topic history first returns to global Topics, then to the previous Space.
  await expect(page.getByRole("heading", { level: 1, name: "Topics", exact: true })).toBeVisible();
  await page.getByRole("group", { name: "History navigation" }).getByRole("button", { name: "Back", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);
  await expect(primaryNavigation.getByRole("button", { name: "Spaces", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("navigation", { name: "Recent spaces" })).toHaveCount(0);
  await expect(spaces).not.toHaveAttribute("aria-pressed");
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
