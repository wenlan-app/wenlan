// SPDX-License-Identifier: AGPL-3.0-only
import { mkdir, writeFile } from "node:fs/promises";
import { expect, test, type Page as BrowserPage } from "@playwright/test";
import type { MilestoneRecord, Page } from "../src/lib/tauri";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";

const title = (page: BrowserPage) => page.getByRole("textbox", { name: "Title", exact: true });
const content = (page: BrowserPage) => page.getByRole("textbox", { name: "Content", exact: true });
const sidebar = (page: BrowserPage) => page.locator('aside[aria-label="Primary navigation"]');

function syntheticPage(id: string, name: string, body: string, status: "active" | "draft" = "draft"): Page {
  return {
    id, title: name, content: body, summary: null, entity_id: null, domain: null,
    space: null, source_memory_ids: [], version: 1, status, creation_kind: "authored",
    review_status: "unconfirmed", created_at: "2026-10-05T12:00:00Z",
    last_compiled: "2026-10-05T12:00:00Z", last_modified: "2026-10-05T12:00:00Z",
  };
}

async function checkShell(page: BrowserPage) {
  await expect(page.locator("main")).toBeVisible();
  await expect(page.locator("vite-error-overlay")).toHaveCount(0);
  expect(await page.title()).toMatch(/Wenlan/);
  await page.evaluate(async () => { await document.fonts.ready; });
}

async function showSidebar(page: BrowserPage) {
  if (await sidebar(page).getAttribute("aria-hidden") === "true") {
    await page.getByTitle("Show sidebar", { exact: true }).click();
  }
  await expect(sidebar(page)).toHaveAttribute("aria-hidden", "false");
}

async function storedPage(page: BrowserPage, id: string) {
  return page.evaluate(async (pageId) =>
    window.__TAURI_INTERNALS__!.invoke("get_page", { id: pageId }) as Promise<Page | null>,
  id);
}

for (const persisted of [false, true]) {
  test(`New page separates a ${persisted ? "persisted" : "not-yet-persisted"} draft's content and identity`, async ({ page }) => {
    await mkdir(test.info().outputDir, { recursive: true });
    const errors = collectBrowserErrors(page);
    const originalId = "page_00000000-0000-4000-8000-000000000001";
    const original = syntheticPage(originalId, "Original synthetic page", "Original synthetic body");
    const controller = await installTauriMock(page, {
      locale: "en", rawActions: [],
      fixture: { ...createSpacesNavigationFixture(), pages: persisted ? [original] : [] },
      localStorage: { "wenlan-theme": "dark" },
    });
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto("/");
    await checkShell(page);
    if (persisted) {
      await openWikiNote(page, "Original synthetic page");
      await expect(title(page)).toHaveValue(original.title);
    } else {
      await page.getByRole("button", { name: "New page", exact: true }).click();
      await title(page).fill(original.title);
      await content(page).fill(original.content);
      // Click before the debounce: the transition itself must flush this editor.
      expect(controller.calls().filter(call => call.command === "create_page_draft")).toHaveLength(0);
    }
    await page.getByRole("button", { name: "New page", exact: true }).click();
    await expect(title(page)).toHaveValue("");
    await expect(content(page)).toHaveValue("");
    await expect(title(page)).toBeFocused();
    const oldCreate = controller.calls().find(call => call.command === "create_page_draft");
    const oldId = persisted ? originalId : (oldCreate!.args as { clientDraftId: string }).clientDraftId;
    await page.screenshot({ path: test.info().outputPath(`new-from-${persisted ? "persisted" : "unpersisted"}-blank.png`) });
    await title(page).fill("Second synthetic page");
    await content(page).fill("Second independent body");
    await expect.poll(() => controller.calls().filter(call => call.command === "create_page_draft").length).toBe(persisted ? 1 : 2);
    const creates = controller.calls().filter(call => call.command === "create_page_draft");
    const newId = (creates.at(-1)!.args as { clientDraftId: string }).clientDraftId;
    expect(newId).not.toBe(oldId);
    expect(await storedPage(page, oldId)).toMatchObject({ title: original.title, content: original.content });
    expect(await storedPage(page, newId)).toMatchObject({ title: "Second synthetic page", content: "Second independent body" });
    expect(controller.calls().filter(call => call.command === "update_page_draft" && (call.args as { id: string }).id === oldId)).toHaveLength(0);
    await page.screenshot({ path: test.info().outputPath(`new-from-${persisted ? "persisted" : "unpersisted"}-saved.png`) });
    await writeFile(test.info().outputPath(`identity-${persisted ? "persisted" : "unpersisted"}.json`), JSON.stringify({ oldId, newId, calls: controller.calls(), errors }, null, 2));
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}

for (const viewport of [{ width: 1280, height: 900 }, { width: 375, height: 812 }]) {
  test(`More and account Escape retain the page draft and drawer at ${viewport.width}px`, async ({ page }) => {
    await mkdir(test.info().outputDir, { recursive: true });
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, {
      locale: "en", rawActions: [], localStorage: { "wenlan-theme": "dark" },
      fixture: { ...createSpacesNavigationFixture(), pages: [] },
    });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await checkShell(page);
    await showSidebar(page);
    if (await sidebar(page).getAttribute("data-sidebar-overlay") === "true") {
      await page.locator('[data-sidebar-toggle="true"]').click();
      await expect(sidebar(page)).toHaveAttribute("aria-hidden", "true");
    }
    await page.getByRole("button", { name: "New page", exact: true }).click();
    await title(page).fill("Popup Escape synthetic draft");
    await content(page).fill("The writing session stays here.");
    await showSidebar(page);

    const more = sidebar(page).getByRole("button", { name: "More", exact: true });
    await more.click();
    await expect(page.getByRole("group", { name: "More", exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("group", { name: "More", exact: true })).toHaveCount(0);
    await expect(more).toBeFocused();
    await expect(sidebar(page)).toHaveAttribute("aria-hidden", "false");
    await expect(title(page)).toHaveValue("Popup Escape synthetic draft");
    await expect(content(page)).toHaveValue("The writing session stays here.");
    await page.screenshot({ path: test.info().outputPath(`more-escape-${viewport.width}.png`) });

    const account = sidebar(page).getByRole("button", { name: "Account menu", exact: true });
    await account.click();
    await expect(page.getByRole("menu", { name: "Account menu", exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu", { name: "Account menu", exact: true })).toHaveCount(0);
    await expect(account).toBeFocused();
    await expect(sidebar(page)).toHaveAttribute("aria-hidden", "false");
    await expect(title(page)).toHaveValue("Popup Escape synthetic draft");
    await expect(content(page)).toHaveValue("The writing session stays here.");
    await expect(page.locator("vite-error-overlay")).toHaveCount(0);
    await page.screenshot({ path: test.info().outputPath(`account-escape-${viewport.width}.png`) });
    await writeFile(test.info().outputPath(`popup-${viewport.width}.json`), JSON.stringify({ viewport, calls: controller.calls(), errors }, null, 2));
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}

test("late first-page milestone resolves only its passive target after an empty Wiki snapshot", async ({ page }) => {
  await mkdir(test.info().outputDir, { recursive: true });
  const errors = collectBrowserErrors(page);
  const target = syntheticPage("page_background_first", "Background synthetic first page", "Background evidence", "active");
  const milestone: MilestoneRecord = { id: "first-concept", acknowledged_at: null, first_triggered_at: 1, payload: { page_id: target.id } };
  const state = { milestones: [] as MilestoneRecord[], available: false, calls: [] as { command: string; args: unknown }[] };
  const controller = await installTauriMock(page, {
    locale: "en", rawActions: [], localStorage: { "wenlan-theme": "dark" },
    fixture: { ...createSpacesNavigationFixture(), pages: [] },
  });
  // Extend only the documented synthetic Tauri boundary; no React state or
  // cache internals are changed to manufacture the late milestone.
  await page.exposeBinding("__pr4MilestoneInvoke", (_source, command: string, args: unknown) => {
    state.calls.push({ command, args });
    if (command === "list_onboarding_milestones") return state.milestones;
    if (command === "get_page") return state.available ? target : null;
    if (command === "acknowledge_onboarding_milestone") {
      state.milestones = [{ ...milestone, acknowledged_at: 2 }];
      return null;
    }
    throw new Error("Unexpected milestone fixture command: " + command);
  });
  await page.addInitScript((targetId) => {
    const internals = window.__TAURI_INTERNALS__!;
    const original = internals.invoke;
    internals.invoke = async (command, args) => {
      if (command === "list_onboarding_milestones" || command === "acknowledge_onboarding_milestone"
        || (command === "get_page" && typeof args === "object" && args !== null && Reflect.get(args, "id") === targetId)) {
        return Reflect.get(window, "__pr4MilestoneInvoke")(command, args);
      }
      return original(command, args);
    };
  }, target.id);
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await checkShell(page);
  await expect(page.getByRole("heading", { name: "Open a page", exact: true, level: 1 })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const initialExplicit = controller.calls().filter(call => call.command.endsWith("_explicit_browse")).length;
  await page.screenshot({ path: test.info().outputPath("milestone-before-empty-wiki.png") });
  state.milestones = [milestone];
  await page.evaluate(async (payload) => {
    await window.__TAURI_INTERNALS__!.invoke("plugin:event|emit", { event: "onboarding-milestone", payload });
  }, milestone);
  await expect.poll(() => state.calls.filter(call => call.command === "get_page").length).toBe(1);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  state.available = true;
  const modal = page.getByRole("dialog", { name: target.title, exact: true });
  await expect(modal).toBeVisible();
  expect(state.calls.filter(call => call.command === "get_page").every(call => (call.args as { id: string }).id === target.id)).toBe(true);
  expect(controller.calls().filter(call => call.command.endsWith("_explicit_browse"))).toHaveLength(initialExplicit);
  expect(state.calls.filter(call => call.command === "acknowledge_onboarding_milestone")).toHaveLength(0);
  const foundCount = state.calls.filter(call => call.command === "get_page").length;
  await page.screenshot({ path: test.info().outputPath("milestone-after-passive-target.png") });
  await modal.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(modal).toHaveCount(0);
  await expect.poll(() => state.calls.filter(call => call.command === "acknowledge_onboarding_milestone").length).toBe(1);
  await page.waitForTimeout(2_200);
  expect(state.calls.filter(call => call.command === "get_page")).toHaveLength(foundCount);
  expect(controller.calls().filter(call => call.command.endsWith("_explicit_browse"))).toHaveLength(initialExplicit);
  await writeFile(test.info().outputPath("milestone-commands.json"), JSON.stringify({ initialExplicit, fixtureCalls: state.calls, calls: controller.calls(), errors }, null, 2));
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
