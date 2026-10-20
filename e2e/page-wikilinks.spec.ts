// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator, type Page } from "@playwright/test";
import type { Page as WenlanPage, UpdatePageInput } from "../src/lib/tauri";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { installTauriMock, type TauriMockController } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

const SOURCE_ID = "page-wikilinks-source";
const TARGET_ID = "page-wikilinks-target";
const SOURCE_TITLE = "Source Wiki links";
const TARGET_TITLE = "Target Link";
const INITIAL_SOURCE = `# ${SOURCE_TITLE}\n\nOpen [[Target Link|target alias]]. See [[Target Link#heading|城市散步]]. Missing [[Missing Page]].\n`;
const AUTOSAVE_TIMEOUT = { timeout: 10_000 };
const PAGE_READ_COMMANDS = new Set(["get_page", "get_page_explicit_browse"]);

type CodeMirrorContent = HTMLElement & {
  cmTile?: { root?: { view?: { state: { doc: { toString(): string; length: number } }; dispatch(spec: unknown): void; focus(): void } } };
  __wikiLinkSession?: boolean;
};

function pageFixture(id: string, title: string, content: string, version: number, summary: string | null = null): WenlanPage {
  return {
    id, title, summary, content, entity_id: null,
    domain: "Wenlan", space: "Wenlan", source_memory_ids: [], version,
    status: "active", creation_kind: "authored", review_status: "confirmed",
    created_at: "2026-10-06T12:00:00.000Z", last_compiled: "2026-10-06T12:00:00.000Z",
    last_modified: "2026-10-06T12:00:00.000Z", user_edited: false,
  };
}

async function openSourcePage(page: Page, delays?: Record<string, number>) {
  const controller = await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    ...(delays ? { delays } : {}),
    fixture: {
      ...createSpacesNavigationFixture(),
      pages: [
        pageFixture(SOURCE_ID, SOURCE_TITLE, INITIAL_SOURCE, 7),
        pageFixture(TARGET_ID, TARGET_TITLE, "# Target Link\n\nA hidden excerpt in the body.", 4, "Short target preview with useful details."),
      ],
    },
  });
  // Keep this fixture local to the scenario: get_page_links is resolved from
  // the daemon's exact outgoing target id, never from a title search.
  await page.addInitScript(({ sourceId, targetId }) => {
    const api = window.__TAURI_INTERNALS__;
    if (!api) return;
    const invoke = api.invoke.bind(api);
    api.invoke = async (command, args, options) => {
      if (command === "get_page_links" && typeof args === "object" && args !== null && Reflect.get(args, "pageId") === sourceId) {
        return { outbound: [{ label: "Target Link", target_page_id: targetId }], inbound: [] };
      }
      return invoke(command, args, options);
    };
  }, { sourceId: SOURCE_ID, targetId: TARGET_ID });
  await page.goto("/");
  await openPrimaryDestination(page, "Wiki");
  await openWikiNote(page, SOURCE_TITLE);
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  await page.mouse.move(1, 1);
  await expect.poll(() => editorSource(editor)).toBe(INITIAL_SOURCE);
  await editor.evaluate((element) => { (element as CodeMirrorContent).__wikiLinkSession = true; });
  return { controller, editor };
}

async function editorSource(editor: Locator): Promise<string> {
  return editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    return view.state.doc.toString();
  });
}

async function sameEditorSession(editor: Locator): Promise<boolean> {
  return editor.evaluate((element) => (element as CodeMirrorContent).__wikiLinkSession === true);
}

async function moveCaretToDocumentEnd(editor: Locator): Promise<void> {
  await editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    view.dispatch({ selection: { anchor: view.state.doc.length }, scrollIntoView: true });
    view.focus();
  });
}

function updateCalls(controller: TauriMockController): UpdatePageInput[] {
  return controller.calls().filter((call) => call.command === "update_page").map((call) => call.args as UpdatePageInput);
}

function canonicalReadsAfterUpdate(controller: TauriMockController, updateNumber: number): number {
  let updates = 0;
  let reads = 0;
  for (const call of controller.calls()) {
    if (call.command === "update_page") { updates++; continue; }
    if (updates === updateNumber && PAGE_READ_COMMANDS.has(call.command)
      && (call.args as { id?: string } | undefined)?.id === SOURCE_ID) reads++;
  }
  return reads;
}

async function expectCanonicalSave(controller: TauriMockController, content: string, expectedVersion: number): Promise<void> {
  await expect.poll(() => {
    const calls = updateCalls(controller);
    return calls[calls.length - 1]?.content;
  }, AUTOSAVE_TIMEOUT).toBe(content);
  const calls = updateCalls(controller);
  const request = calls[calls.length - 1];
  expect(request).toMatchObject({ id: SOURCE_ID, content, expectedVersion, callerId: "wenlan-app" });
  expect(request?.operationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
  await expect.poll(() => canonicalReadsAfterUpdate(controller, updateCalls(controller).length), AUTOSAVE_TIMEOUT).toBeGreaterThan(0);
}

test("resolved wikilink previews on hover and opens from the preview card", async ({ page }) => {
  const { controller, editor } = await openSourcePage(page);
  const link = page.getByRole("link", { name: "target alias", exact: true });
  await expect(link).toHaveAttribute("href", `#concept:${TARGET_ID}`);
  await expect(page.getByRole("link", { name: "Missing Page", exact: true })).toHaveCount(0);

  await link.hover();
  const preview = page.getByRole("dialog", { name: /Target Link/ });
  await expect(preview).toBeVisible();
  await expect(preview).toContainText("Short target preview with useful details.");
  expect(await sameEditorSession(editor)).toBe(true);
  await expect(controller.calls().filter((call) => call.command === "get_page_explicit_browse"
    && (call.args as { id?: string } | undefined)?.id === TARGET_ID)).toHaveLength(0);

  await preview.hover();
  await page.waitForTimeout(250);
  await expect(preview).toBeVisible();
  await preview.getByRole("button", { name: "Open Target Link", exact: true }).click();
  await expect(page.locator(".page-detail")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Page editor", exact: true })).toBeVisible();
  await expect.poll(() => editorSource(page.getByRole("textbox", { name: "Page editor", exact: true })))
    .toContain("# Target Link");
  expect(updateCalls(controller)).toHaveLength(0);
});

test("a real wikilink click flushes dirty source before opening its exact target", async ({ page }) => {
  const { controller, editor } = await openSourcePage(page, {
    update_page: 350,
    get_page_explicit_browse: 200,
  });
  await moveCaretToDocumentEnd(editor);
  await page.keyboard.insertText(" Unsaved tail.");
  const dirtySource = `${INITIAL_SOURCE} Unsaved tail.`;
  await expect.poll(() => editorSource(editor)).toBe(dirtySource);
  const link = page.getByRole("link", { name: "target alias", exact: true });
  await link.click();

  await expect(page.getByRole("textbox", { name: "Page editor", exact: true })).toBeVisible();
  await expect.poll(() => editorSource(page.getByRole("textbox", { name: "Page editor", exact: true })))
    .toContain("# Target Link");
  await expectCanonicalSave(controller, dirtySource, 7);
  const calls = controller.calls();
  const updateIndex = calls.findIndex((call) => call.command === "update_page");
  expect(updateIndex).toBeGreaterThanOrEqual(0);
  const sourceReadbackIndex = calls.findIndex((call, index) => index > updateIndex
    && PAGE_READ_COMMANDS.has(call.command)
    && (call.args as { id?: string } | undefined)?.id === SOURCE_ID);
  const targetOpenIndex = calls.findIndex((call) => call.command === "get_page_explicit_browse"
    && (call.args as { id?: string } | undefined)?.id === TARGET_ID);
  expect(sourceReadbackIndex).toBeGreaterThan(updateIndex);
  expect(targetOpenIndex).toBeGreaterThan(sourceReadbackIndex);
  expect(await sameEditorSession(editor)).toBe(false);
});

test("a failed save keeps the wikilink source and unsaved edits in the same editor", async ({ page }) => {
  const { controller, editor } = await openSourcePage(page);
  const failedSource = `${INITIAL_SOURCE} Must remain here.`;
  await moveCaretToDocumentEnd(editor);
  await page.keyboard.insertText(" Must remain here.");
  await expect.poll(() => editorSource(editor)).toBe(failedSource);
  controller.failNext("update_page", "simulated link navigation save failure");
  await page.getByRole("link", { name: "target alias", exact: true }).click();

  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(1);
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  await page.mouse.move(1, 1);
  await expect.poll(() => editorSource(editor)).toBe(failedSource);
  expect(await sameEditorSession(editor)).toBe(true);
  expect(controller.calls().some((call) => call.command === "get_page_explicit_browse"
    && (call.args as { id?: string } | undefined)?.id === TARGET_ID)).toBe(false);
});

test("Enter opens a focused wikilink without inserting a newline", async ({ page }) => {
  const { controller } = await openSourcePage(page);
  const link = page.getByRole("link", { name: "target alias", exact: true });
  await link.focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => editorSource(page.getByRole("textbox", { name: "Page editor", exact: true })))
    .toContain("# Target Link");
  expect(updateCalls(controller)).toHaveLength(0);
  expect(controller.calls().some((call) => call.command === "get_page_explicit_browse"
    && (call.args as { id?: string } | undefined)?.id === TARGET_ID)).toBe(true);
});

test("Escape from the keyboard preview returns to the same editable source link", async ({ page }) => {
  const { editor } = await openSourcePage(page);
  const link = page.getByRole("link", { name: "target alias", exact: true });
  await link.focus();
  const preview = page.getByRole("dialog", { name: /Target Link/ });
  await expect(preview).toBeVisible();
  await page.keyboard.press("Tab");
  const open = preview.getByRole("button", { name: "Open Target Link", exact: true });
  await expect(open).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(preview).toHaveCount(0);
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  await expect(link).toBeFocused();
  expect(await sameEditorSession(editor)).toBe(true);
  expect(await editorSource(editor)).toBe(INITIAL_SOURCE);
  await page.waitForTimeout(240);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("resolved link syntax is concealed outside its active editing range without changing source", async ({ page }) => {
  const { editor } = await openSourcePage(page);
  const resolved = "[[Target Link|target alias]]";
  const sourceToken = await editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    const source = view.state.doc.toString();
    return { from: source.indexOf("[[Target Link|target alias]]"), to: source.indexOf("[[Target Link|target alias]]") + "[[Target Link|target alias]]".length };
  });
  await editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    view?.dispatch({ selection: { anchor: 0 } });
    view?.focus();
  });
  await expect(editor).toContainText("Open target alias. See 城市散步. Missing [[Missing Page]].");
  await expect(editor).not.toContainText(resolved);
  expect(await editorSource(editor)).toBe(INITIAL_SOURCE);

  await editor.evaluate((element, from) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    view?.dispatch({ selection: { anchor: from + 4 } });
    view?.focus();
  }, sourceToken.from);
  await expect(editor).toContainText(resolved);
  expect(await editorSource(editor)).toBe(INITIAL_SOURCE);

  await editor.evaluate((element, to) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    view?.dispatch({ selection: { anchor: to } });
    view?.focus();
  }, sourceToken.to);
  await expect(editor).not.toContainText(resolved);
  expect(await editorSource(editor)).toBe(INITIAL_SOURCE);
});

test("completing a resolved wikilink conceals syntax as soon as the caret passes the closing brackets", async ({ page }) => {
  const { editor } = await openSourcePage(page);
  await moveCaretToDocumentEnd(editor);
  await page.keyboard.insertText(" [[Target Link|completed]]");
  const expected = `${INITIAL_SOURCE} [[Target Link|completed]]`;
  await expect.poll(() => editorSource(editor)).toBe(expected);
  await expect(editor).toContainText("completed");
  await expect(editor).not.toContainText("[[Target Link|completed]]");

  const tokenFrom = expected.indexOf("[[Target Link|completed]]");
  await editor.evaluate((element, from) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    view?.dispatch({ selection: { anchor: from + 4 } });
    view?.focus();
  }, tokenFrom);
  await expect(editor).toContainText("[[Target Link|completed]]");
  expect(await editorSource(editor)).toBe(expected);

  await moveCaretToDocumentEnd(editor);
  await expect(editor).not.toContainText("[[Target Link|completed]]");
  expect(await editorSource(editor)).toBe(expected);
});

test("native drag selection and Alt-click on a concealed link remain editing actions", async ({ page }) => {
  const { controller, editor } = await openSourcePage(page);
  const link = page.getByRole("link", { name: "target alias", exact: true });
  const bounds = await link.evaluate((element) => {
    const rect = document.createRange();
    rect.selectNodeContents(element);
    const box = rect.getBoundingClientRect();
    const inset = Math.max(1, box.width / (element.textContent?.length || 1) / 4);
    return { left: box.left + inset, right: box.right - inset, y: box.top + box.height / 2 };
  });
  await page.mouse.move(bounds.left, bounds.y);
  await page.mouse.down();
  await page.mouse.move(bounds.right, bounds.y, { steps: 6 });
  await page.mouse.up();
  const selected = await page.evaluate(() => window.getSelection()?.toString() ?? "");
  expect(selected).toContain("target");
  await expect(editor).toBeVisible();
  expect(await sameEditorSession(editor)).toBe(true);
  expect(await editorSource(editor)).toBe(INITIAL_SOURCE);
  expect(controller.calls().some((call) => call.command === "get_page_explicit_browse"
    && (call.args as { id?: string } | undefined)?.id === TARGET_ID)).toBe(false);

  await link.click({ modifiers: ["Alt"] });
  await expect(editor).toBeVisible();
  await expect(editor).toContainText("[[Target Link|target alias]]");
  expect(await sameEditorSession(editor)).toBe(true);
  expect(await editorSource(editor)).toBe(INITIAL_SOURCE);
  expect(controller.calls().some((call) => call.command === "get_page_explicit_browse"
    && (call.args as { id?: string } | undefined)?.id === TARGET_ID)).toBe(false);
});
