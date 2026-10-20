// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator, type Page } from "@playwright/test";
import { resources } from "../src/i18n/resources";
import type { Page as WenlanPage, UpdatePageInput } from "../src/lib/tauri";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { installTauriMock, type TauriMockController } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

const PAGE_ID = "page-slash-editor-e2e";
const PAGE_TITLE = "Slash editor fixture";
const INITIAL_SOURCE = `# ${PAGE_TITLE}\n\nExisting paragraph.\n\n`;
const labels = resources.en.translation.slashEditing;
const OPTION_LABELS = [
  labels.heading1, labels.heading2, labels.bulletList, labels.numberedList,
  labels.taskList, labels.blockquote, labels.fencedCode,
];
const AUTOSAVE_TIMEOUT = { timeout: 10_000 };
const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const PAGE_READ_COMMANDS = new Set(["get_page", "get_page_explicit_browse"]);

// Same live-view seam used by page-editor.spec.ts. Model reads avoid confusing
// contextual Markdown decorations or virtualized lines with the stored source.
type BrowserEditorView = {
  state: {
    doc: { length: number; toString(): string };
    selection: { main: { anchor: number; head: number } };
  };
  dispatch(spec: { selection: { anchor: number }; scrollIntoView: boolean }): void;
  focus(): void;
};
type CodeMirrorContent = HTMLElement & {
  cmTile?: { root?: { view?: BrowserEditorView } };
  __slashTestSession?: boolean;
};

function pageFixture(content: string): WenlanPage {
  return {
    id: PAGE_ID, title: PAGE_TITLE, summary: null, content, entity_id: null,
    domain: "Wenlan", space: "Wenlan", source_memory_ids: [], version: 7,
    status: "active", creation_kind: "authored", review_status: "confirmed",
    created_at: "2026-10-05T12:00:00.000Z",
    last_compiled: "2026-10-05T12:00:00.000Z",
    last_modified: "2026-10-05T12:00:00.000Z", user_edited: false,
  };
}

async function openEditor(page: Page, source = INITIAL_SOURCE) {
  const controller = await installTauriMock(page, {
    locale: "en", rawActions: [],
    fixture: { ...createSpacesNavigationFixture(), pages: [pageFixture(source)] },
  });
  await page.goto("/");
  await openPrimaryDestination(page, "Wiki");
  await openWikiNote(page, PAGE_TITLE);
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  await expect(editor).toHaveClass(/cm-content/); // Slash is CodeMirror-only.
  await expect.poll(() => editorSource(editor)).toBe(source);
  await editor.evaluate((element) => {
    (element as CodeMirrorContent).__slashTestSession = true;
  });
  await expect(page.getByRole("toolbar", { name: "Formatting", exact: true })).toHaveCount(0);
  return { controller, editor };
}

async function editorSource(editor: Locator): Promise<string> {
  return editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    return view.state.doc.toString();
  });
}

async function sameSession(editor: Locator): Promise<boolean> {
  return editor.evaluate((element) => (element as CodeMirrorContent).__slashTestSession === true);
}

async function caret(editor: Locator): Promise<number> {
  return editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    if (view.state.selection.main.anchor !== view.state.selection.main.head) throw new Error("Expected a single caret");
    return view.state.selection.main.head;
  });
}

async function moveToEnd(editor: Locator): Promise<void> {
  await editor.evaluate((element) => {
    const view = (element as CodeMirrorContent).cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    // Selection only: this must not manufacture a typed-input transaction.
    view.dispatch({ selection: { anchor: view.state.doc.length }, scrollIntoView: true });
    view.focus();
  });
}

function updateCalls(controller: TauriMockController): UpdatePageInput[] {
  return controller.calls().filter((call) => call.command === "update_page")
    .map((call) => call.args as UpdatePageInput);
}

function canonicalReadsAfterUpdate(controller: TauriMockController, updateNumber: number): number {
  let updates = 0;
  let reads = 0;
  for (const call of controller.calls()) {
    if (call.command === "update_page") { updates++; continue; }
    if (updates === updateNumber && PAGE_READ_COMMANDS.has(call.command)
      && (call.args as { id?: string } | undefined)?.id === PAGE_ID) reads++;
  }
  return reads;
}

async function expectAutosaved(page: Page, controller: TauriMockController, source: string): Promise<void> {
  await expect.poll(() => updateCalls(controller).slice(-1)[0]?.content, AUTOSAVE_TIMEOUT).toBe(source);
  await expect(page.locator(".page-detail").getByRole("status")
    .filter({ hasText: new RegExp(`^${resources.en.translation.pageDetail.editor.saved}$`) }))
    .toBeVisible(AUTOSAVE_TIMEOUT);
  const writes = updateCalls(controller);
  // Intermediate literal slash/skeleton writes are legal if a browser step
  // spans the debounce. Every subsequent snapshot must use the confirmed CAS.
  for (const [index, request] of writes.entries()) {
    expect(request).toMatchObject({ id: PAGE_ID, expectedVersion: 7 + index, callerId: "wenlan-app" });
    expect(request.operationId).toMatch(UUID_V4);
    expect(canonicalReadsAfterUpdate(controller, index + 1)).toBeGreaterThan(0);
  }
  expect(new Set(writes.map((request) => request.operationId)).size).toBe(writes.length);
  const stored = await page.evaluate(async (id) => window.__TAURI_INTERNALS__!.invoke("get_page", { id }), PAGE_ID);
  expect(stored).toMatchObject({ content: source, version: 7 + writes.length, user_edited: true });
}

async function typedMenu(page: Page, editor: Locator): Promise<Locator> {
  await moveToEnd(editor);
  await editor.press("/");
  const menu = page.getByRole("listbox", { name: labels.label, exact: true });
  await expect(menu).toBeVisible();
  // Exact inventory excludes AI actions and catches raw untranslated keys.
  await expect(menu.getByRole("option")).toHaveText(OPTION_LABELS);
  await expect(editor).toBeFocused();
  await expect(editor).toHaveAttribute("aria-controls", await menu.getAttribute("id") as string);
  return menu;
}

test("keyboard slash selection replaces one slash and autosaves on the confirmed CAS version", async ({ page }) => {
  const { controller, editor } = await openEditor(page);
  // Keep the parked pointer away from the popup: pointerenter intentionally
  // changes the active item, while this scenario exercises keyboard selection.
  await page.mouse.move(0, 0);
  const menu = await typedMenu(page, editor);
  await expectAutosaved(page, controller, `${INITIAL_SOURCE}/`);
  expect(updateCalls(controller)).toHaveLength(1);
  // Waited slash readback must not close or reopen the current menu/session.
  await expect(menu).toBeVisible();
  expect(await sameSession(editor)).toBe(true);
  await expect(menu.getByRole("option", { name: labels.heading1, exact: true })).toHaveAttribute("aria-selected", "true");
  for (let step = 0; step < 4; step++) await editor.press("ArrowDown");
  const selected = menu.getByRole("option", { name: labels.taskList, exact: true });
  await expect(selected).toHaveAttribute("aria-selected", "true");
  await expect(editor).toHaveAttribute("aria-activedescendant", await selected.getAttribute("id") as string);
  await editor.press("Enter");
  await expect(menu).toHaveCount(0);
  await expect.poll(() => editorSource(editor)).toBe(`${INITIAL_SOURCE}- [ ] `);
  expect(await caret(editor)).toBe(INITIAL_SOURCE.length + 6);
  await page.keyboard.insertText("Check facts");
  const source = `${INITIAL_SOURCE}- [ ] Check facts`;
  await expectAutosaved(page, controller, source);
  expect(updateCalls(controller)[1]).toMatchObject({ expectedVersion: 8 });
  expect(updateCalls(controller).slice(-1)[0]).toMatchObject({ content: source });
  expect(await editorSource(editor)).toBe(source);
  expect(await sameSession(editor)).toBe(true);
});

test("reopening slash after autosave keeps its final option visible and clickable", async ({ page }) => {
  const { controller, editor } = await openEditor(page);
  await page.mouse.move(0, 0);
  const slashSource = `${INITIAL_SOURCE}/`;

  for (let opening = 0; opening < 3; opening++) {
    const menu = await typedMenu(page, editor);
    await expectAutosaved(page, controller, slashSource);
    expect(await editorSource(editor)).toBe(slashSource);
    expect(await sameSession(editor)).toBe(true);

    const finalOption = menu.getByRole("option", { name: labels.fencedCode, exact: true });
    // Safari remeasures CodeMirror tooltips shortly after the canonical
    // readback. Check settled geometry and the actual pointer hit target.
    await page.waitForTimeout(150);
    await expect.poll(async () => {
      const box = await finalOption.boundingBox();
      if (!box) return false;
      const viewport = page.viewportSize()!;
      const inViewport = box.x >= 0 && box.y >= 0
        && box.x + box.width <= viewport.width
        && box.y + box.height <= viewport.height;
      if (!inViewport) return false;
      return finalOption.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
        return hit === element || (hit !== null && element.contains(hit));
      });
    }, { timeout: 5_000, intervals: [100] }).toBe(true);
    await expect(finalOption).toBeInViewport();

    if (opening < 2) {
      await editor.press("Escape");
      await expect(menu).toHaveCount(0);
      await expect(editor).toBeFocused();
      expect(await editorSource(editor)).toBe(slashSource);
      expect(await sameSession(editor)).toBe(true);
      await editor.press("Backspace");
      await expect.poll(() => editorSource(editor)).toBe(INITIAL_SOURCE);
      await expectAutosaved(page, controller, INITIAL_SOURCE);
      expect(await sameSession(editor)).toBe(true);
      continue;
    }

    // This is a normal pointer action after explicit elementFromPoint hit
    // testing; do not force the click through a clipped menu.
    await finalOption.click();
    await expect(menu).toHaveCount(0);
    await expect(editor).toBeEditable();
    await expect(editor).toBeFocused();
    const codeBlock = `${INITIAL_SOURCE}\`\`\`\n\n\`\`\``;
    await expect.poll(() => editorSource(editor)).toBe(codeBlock);
    expect(await sameSession(editor)).toBe(true);
    await page.keyboard.insertText('console.log("reopened");');
    const finalSource = `${INITIAL_SOURCE}\`\`\`\nconsole.log("reopened");\n\`\`\``;
    await expectAutosaved(page, controller, finalSource);
    expect(await editorSource(editor)).toBe(finalSource);
    expect(await sameSession(editor)).toBe(true);
  }
});

test("an initial slash stays literal and Escape dismisses only the typed menu in the same editor", async ({ page }) => {
  const source = `${INITIAL_SOURCE}/`;
  const { controller, editor } = await openEditor(page, source);
  await moveToEnd(editor);
  await expect(page.getByRole("listbox", { name: labels.label, exact: true })).toHaveCount(0);
  expect(updateCalls(controller)).toHaveLength(0);
  await editor.press("Backspace");
  await expect.poll(() => editorSource(editor)).toBe(INITIAL_SOURCE);
  const menu = await typedMenu(page, editor);
  await editor.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(editor).toBeEditable();
  await expect(editor).toBeFocused();
  expect(await editorSource(editor)).toBe(source);
  expect(await sameSession(editor)).toBe(true);
  await page.keyboard.insertText(" is literal text.");
  await expect(page.getByRole("listbox", { name: labels.label, exact: true })).toHaveCount(0);
  const finalSource = `${source} is literal text.`;
  await expectAutosaved(page, controller, finalSource);
  expect(await sameSession(editor)).toBe(true);
});

test("Chinese composition closes slash without stealing candidate keys or losing the final source", async ({ page }) => {
  const { controller, editor } = await openEditor(page);
  const menu = await typedMenu(page, editor);
  await expectAutosaved(page, controller, `${INITIAL_SOURCE}/`);
  const writesBeforeComposition = updateCalls(controller).length;
  // Synthetic DOM composition follows the existing real-editor unit test's
  // browser event order. This is not a claim about an installed OS IME.
  const prevented = await editor.evaluate((element) => {
    element.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "中文" }));
    element.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: "中文" }));
    const keys = ["ArrowDown", "Enter", "Escape"].map((key) => {
      const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, isComposing: true, keyCode: 229 });
      element.dispatchEvent(event);
      return event.defaultPrevented;
    });
    element.dispatchEvent(new InputEvent("beforeinput", {
      bubbles: true, cancelable: true, data: "中文", inputType: "insertCompositionText", isComposing: true,
    }));
    const line = element.querySelector(".cm-line:last-child");
    if (!line) throw new Error("Expected the visible final composition line");
    const text = document.createTextNode("中文");
    line.append(text);
    const range = document.createRange();
    range.setStartAfter(text);
    range.collapse(true);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    element.dispatchEvent(new InputEvent("input", {
      bubbles: true, data: "中文", inputType: "insertCompositionText", isComposing: true,
    }));
    return keys;
  });
  expect(prevented).toEqual([false, false, false]);
  await expect(menu).toHaveCount(0);
  await expect(editor).toBeFocused();
  // Past the 650 ms debounce: unfinished composition must not write a partial
  // candidate or flush/exit because its Enter/Escape was intercepted.
  await page.waitForTimeout(850);
  expect(updateCalls(controller)).toHaveLength(writesBeforeComposition);
  expect(await sameSession(editor)).toBe(true);
  await editor.evaluate((element) => {
    element.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "中文" }));
  });
  const finalSource = `${INITIAL_SOURCE}/中文`;
  await expect.poll(() => editorSource(editor)).toBe(finalSource);
  await expect(page.getByRole("listbox", { name: labels.label, exact: true })).toHaveCount(0);
  await expectAutosaved(page, controller, finalSource);
  expect(await sameSession(editor)).toBe(true);
});

test("mouse code-block selection replaces only slash and keeps the caret ready for typing", async ({ page }) => {
  const { controller, editor } = await openEditor(page);
  const menu = await typedMenu(page, editor);
  await menu.getByRole("option", { name: labels.fencedCode, exact: true }).click();
  await expect(menu).toHaveCount(0);
  await expect(editor).toBeFocused();
  await expect.poll(() => editorSource(editor)).toBe(`${INITIAL_SOURCE}\`\`\`\n\n\`\`\``);
  expect(await caret(editor)).toBe(INITIAL_SOURCE.length + 4);
  await page.keyboard.insertText('console.log("ok");');
  const source = `${INITIAL_SOURCE}\`\`\`\nconsole.log("ok");\n\`\`\``;
  await expect.poll(() => editorSource(editor)).toBe(source);
  await expectAutosaved(page, controller, source);
  expect(await sameSession(editor)).toBe(true);
});

test("a dark slash popup at the bottom of a long document fits a 375px viewport", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 375, height: 812 });
  const source = [
    `# ${PAGE_TITLE}`, "",
    ...Array.from({ length: 120 }, (_, index) => `Paragraph ${index + 1} has enough text to wrap naturally on a narrow screen.`),
    "", "",
  ].join("\n");
  const { controller, editor } = await openEditor(page, source);
  await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
  const main = page.locator(".wiki-workspace-content");
  await expect.poll(() => main.evaluate((element) => element.scrollHeight - element.clientHeight)).toBeGreaterThan(0);
  const menu = await typedMenu(page, editor);
  await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await editor.press("ArrowUp");
  await expect(menu.getByRole("option", { name: labels.fencedCode, exact: true })).toHaveAttribute("aria-selected", "true");
  await expect.poll(async () => {
    const box = await menu.boundingBox();
    return !!box && box.width > 0 && box.height > 0 && box.x >= 0 && box.y >= 60
      && box.x + box.width <= 375 && box.y + box.height <= 812;
  }).toBe(true);
  await expect(menu.getByRole("option", { name: labels.fencedCode, exact: true })).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  await expect(editor).toBeFocused();
  expect(await editorSource(editor)).toBe(`${source}/`);
  await testInfo.attach("slash-menu-375-dark", { body: await page.screenshot(), contentType: "image/png" });
  await editor.press("Escape");
  await expect(menu).toHaveCount(0);
  expect(await sameSession(editor)).toBe(true);
  await expectAutosaved(page, controller, `${source}/`);
});
