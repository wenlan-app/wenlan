// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

const recentChanges = [{
  page_id: "page-architecture",
  title: "Fixture architecture",
  change_kind: "revised" as const,
  changed_at_ms: Date.parse("2026-07-10T12:00:00Z"),
}];

async function openFixturePage(page: Page, title = "Fixture architecture"): Promise<void> {
  if (page.url() === "about:blank") await page.goto("/");
  await openWikiNote(page, title);
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await editor.press("Escape");
  await expect(editor).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
}

async function chooseRename(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Page actions" }).click();
  await page.getByRole("menuitem", { name: "Rename page" }).click();
}

test("existing linked mentions show the new target title while stored text and aliases stay intact", async ({ page }, testInfo) => {
  const errors = collectBrowserErrors(page);
  const fixture = createSpacesNavigationFixture();
  const content = "Read [[History semantics]] or [[History semantics|my shorthand]].";
  await installTauriMock(page, {
    locale: "en", rawActions: [],
    fixture: { ...fixture, pages: fixture.pages.map((note) => note.id === "page-architecture" ? { ...note, content } : note) },
    pageScenario: {
      recentChanges: [...recentChanges, { page_id: "page-history", title: "History semantics", change_kind: "revised", changed_at_ms: Date.parse("2026-07-10T12:00:00Z") }],
      outboundLinks: { "page-architecture": [{ label: "History semantics", target_page_id: "page-history" }] },
    },
  });
  await openFixturePage(page);
  await expect(page.getByRole("link", { name: "History semantics", exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("linked-title-before.png") });
  await openFixturePage(page, "History semantics");
  await chooseRename(page);
  await page.getByRole("textbox", { name: "Page title" }).fill("Version history guide");
  await page.getByRole("button", { name: "Save title" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Version history guide" })).toBeVisible();
  await openFixturePage(page);
  await expect(page.getByRole("link", { name: "Version history guide", exact: true })).toBeVisible();
  await expect(page.getByRole("link", { name: "History semantics", exact: true })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "my shorthand", exact: true })).toBeVisible();
  const stored = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { content: string };
  expect(stored.content).toBe(content);
  await page.screenshot({ path: testInfo.outputPath("linked-title-after.png") });
  await page.getByRole("link", { name: "Version history guide", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Version history guide" })).toBeVisible();
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("renames an active page while preserving its identity and body after revisiting", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    delays: { rename_page: 600 },
    pageScenario: { recentChanges },
  });
  await openFixturePage(page);
  const before = await page.evaluate(async () =>
    window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" }),
  ) as { id: string; content: string };
  const original = { id: before.id, content: before.content };

  await chooseRename(page);
  await page.getByRole("textbox", { name: "Page title" }).fill("Renamed architecture");
  await page.getByRole("textbox", { name: "Page title" }).press("Enter");
  await expect(page.getByRole("button", { name: "Renaming…" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Cancel rename" })).toBeDisabled();
  await page.getByRole("button", { name: "Page actions" }).click();
  await expect(page.getByRole("menuitem", { name: "Edit page" })).toBeDisabled();
  await page.getByRole("button", { name: "Page actions" }).click();

  await expect(page.getByRole("heading", { level: 1, name: "Renamed architecture" })).toBeVisible();
  const renamed = await page.evaluate(async () =>
    window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" }),
  ) as { id: string; title: string; content: string };
  expect({ id: renamed.id, title: renamed.title, content: renamed.content }).toEqual({
    ...original,
    title: "Renamed architecture",
  });
  expect(controller.calls().some((call) => call.command === "rename_page")).toBe(true);

  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();
  await openWikiNote(page, "Renamed architecture");
  await expect(page.getByRole("heading", { level: 1, name: "Renamed architecture" })).toBeVisible();
  const revisited = await page.evaluate(async () =>
    window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" }),
  ) as { id: string; content: string };
  expect({ id: revisited.id, content: revisited.content }).toEqual(original);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("cancels a page rename without changing the title", async ({ page }) => {
  await installTauriMock(page, { locale: "en", rawActions: [], pageScenario: { recentChanges } });
  await openFixturePage(page);
  await chooseRename(page);
  await page.getByRole("textbox", { name: "Page title" }).fill("Discard this title");
  await page.getByRole("textbox", { name: "Page title" }).press("Escape");
  await expect(page.getByRole("heading", { level: 1, name: "Fixture architecture" })).toBeVisible();
  await page.getByRole("button", { name: "Page actions" }).click();
  await expect(page.getByRole("menuitem", { name: "Rename page" })).toBeVisible();
});

test("keeps a failed page rename retryable with its title and page intact", async ({ page }) => {
  const controller = await installTauriMock(page, { locale: "en", rawActions: [], pageScenario: { recentChanges } });
  controller.failNext("rename_page", "daemon offline");
  await openFixturePage(page);
  await chooseRename(page);
  await page.getByRole("textbox", { name: "Page title" }).fill("Try again later");
  await page.getByRole("button", { name: "Save title" }).click();
  await expect(page.getByRole("alert")).toHaveText(/Could not rename this page/);
  await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue("Try again later");
  await page.getByRole("button", { name: "Save title" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Try again later" })).toBeVisible();
});
