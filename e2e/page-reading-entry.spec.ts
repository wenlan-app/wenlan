// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";
import type { Page as Note } from "../src/lib/tauri";

import type { AppLocale } from "../src/i18n/locales";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";

const TITLE = "Reading page";
const SECOND = "Another page";
const INTRO = `# ${TITLE}\n\n> This quoted passage stays readable.\n\n## Context\n\nA **bold phrase** and [guide](https://example.com/guide).\n\n`;
const SOURCE = INTRO + Array.from({ length: 80 }, (_, i) => `Paragraph ${i + 1}: Keep the reading position when returning to this page.\n`).join("\n");
function note(id: string, title: string, content: string): Note {
  return { id, title, content, summary: null, entity_id: null, domain: "Wenlan", source_memory_ids: [], version: 7, status: "active", creation_kind: "authored", review_status: "confirmed", created_at: "2026-10-07T12:00:00Z", last_compiled: "2026-10-07T12:00:00Z", last_modified: "2026-10-07T12:00:00Z" };
}
async function start(page: Page, locale: AppLocale = "en", theme = "light", width = 1440) {
  await page.setViewportSize({ width, height: 900 });
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { locale, rawActions: [], fixture: { ...createSpacesNavigationFixture(), pages: [note("reading-entry", TITLE, SOURCE), note("reading-other", SECOND, `# ${SECOND}\n\nA second reading surface.`)] }, localStorage: { "wenlan-theme": theme } });
  await page.goto("/");
  await openWikiNote(page, TITLE);
  const editor = page.locator(".cm-content");
  await expect(editor).toContainText("This quoted passage stays readable.");
  await expect(editor).toBeEditable();
  return { editor, runtime, errors };
}
async function source(editor: Locator) {
  return editor.evaluate(el => (el as HTMLElement & { cmTile: { root: { view: { state: { doc: { toString(): string } } } } } }).cmTile.root.view.state.doc.toString());
}
async function expectReading(editor: Locator) {
  await expect(editor).not.toBeFocused();
  await expect(editor.locator(".cm-writing-heading-1")).toHaveText(TITLE);
  await expect(editor.locator(".cm-writing-blockquote")).toHaveText("This quoted passage stays readable.");
  await expect(editor.locator(".cm-writing-link")).toHaveText("guide");
  await expect(editor.locator(".cm-writing-strong")).toHaveText("bold phrase");
}
// Use actual glyph coordinates, not editor.focus()/programmatic selection:
// a first click must land at the visually clicked character before syntax expands.
async function clickQuotedWord(page: Page, editor: Locator) {
  const point = await editor.locator(".cm-writing-blockquote").evaluate(el => {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    let text: Node | null;
    while ((text = walker.nextNode())) {
      const i = text.textContent!.indexOf("stays");
      if (i < 0) continue;
      const range = document.createRange(); range.setStart(text, i); range.setEnd(text, i + 1);
      const rect = range.getBoundingClientRect();
      return { x: rect.left + 1, y: rect.top + rect.height / 2 };
    }
    throw new Error("Expected visible quoted word");
  });
  await page.mouse.click(point.x, point.y);
}

test("existing page opens and hovers without focus, syntax reveal, or writes", async ({ page }) => {
  const { editor, runtime, errors } = await start(page);
  await expectReading(editor);
  await editor.locator(".cm-writing-blockquote").hover();
  await editor.locator(".cm-writing-link").hover();
  await expectReading(editor);
  await page.waitForTimeout(800); // A read/hover must remain quiet beyond the 650ms autosave debounce.
  expect(await source(editor)).toBe(SOURCE);
  expect(runtime.calls().filter(c => c.command === "update_page")).toHaveLength(0);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("first pointer click edits at the clicked glyph, then blur restores typesetting", async ({ page }, info) => {
  const { editor, runtime, errors } = await start(page);
  await expectReading(editor);
  await clickQuotedWord(page, editor);
  await expect(editor).toBeFocused();
  await expect(editor.locator(".cm-writing-blockquote")).toHaveText("> This quoted passage stays readable.");
  await page.keyboard.insertText("HERE ");
  const expected = SOURCE.replace("passage stays", "passage HERE stays");
  await expect.poll(() => source(editor)).toBe(expected);
  await expect.poll(() => runtime.calls().filter(c => c.command === "update_page").length).toBe(1);
  await page.locator(".note-tabs-list [role=tab][aria-selected=true]").click();
  await expect(editor).not.toBeFocused();
  await expect(editor.locator(".cm-writing-blockquote")).toHaveText("This quoted passage HERE stays readable.");
  await page.screenshot({ path: info.outputPath("click-and-blur.png") });
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("tab return preserves reading scroll independently of the remembered caret", async ({ page }, info) => {
  const { editor, runtime, errors } = await start(page);
  await clickQuotedWord(page, editor); // Retain a caret near the top, far from the reading position.
  await page.locator(".note-tabs-list [role=tab][aria-selected=true]").click();
  await expect(editor).not.toBeFocused();
  const reading = page.locator(".wiki-workspace-content");
  await reading.evaluate(el => { el.scrollTop = 1400; });
  await expect.poll(() => reading.evaluate(el => el.scrollTop)).toBe(1400);
  await page.waitForTimeout(80); // Deliver the native scroll event before changing the pane.
  const oldPosition = await reading.evaluate(el => el.scrollTop);
  await openWikiNote(page, SECOND);
  await expect(page.locator(".cm-content")).toContainText("A second reading surface.");
  const tab = page.locator(".note-tabs-list").getByRole("tab", { name: TITLE, exact: true });
  await tab.click();
  await expect(page.locator(".cm-content")).not.toBeFocused();
  await expect.poll(() => reading.evaluate(el => el.scrollTop)).toBeCloseTo(oldPosition, 0);
  await page.waitForTimeout(250); // Catch delayed CodeMirror focus/measurement scrolling.
  expect(await reading.evaluate(el => el.scrollTop)).toBeCloseTo(oldPosition, 0);
  await page.screenshot({ path: info.outputPath("tab-return.png") });
  expect(runtime.calls().filter(c => c.command === "update_page")).toHaveLength(0);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("a new empty page still focuses its title for immediate input", async ({ page }) => {
  await start(page);
  await page.locator(".note-tab-create").click();
  const title = page.getByRole("textbox", { name: "Title", exact: true });
  await expect(title).toBeFocused();
  await page.keyboard.insertText("Start writing");
  await expect(title).toHaveValue("Start writing");
});
