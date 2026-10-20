// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator, type Page } from "@playwright/test";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

function fixture() {
  const base = createReviewDecisionFixture("wiki-folders");
  return { ...base, pages: Array.from({ length: 18 }, (_, i) => ({
    ...base.pages[0], id: `header-${i}`, title: `Layout page ${i + 1}`,
    content: `# Layout page ${i + 1}\n\nA synthetic page for tab overflow.`,
    storage_path: `layout-${i + 1}.md`, folder_path: "",
  })) };
}
const frame = (page: Page, id = "primary") => page.locator(`[data-note-group-id="${id}"]`);

async function geometry(group: Locator) {
  return group.evaluate(el => {
    const rect = (selector: string) => {
      const r = el.querySelector(selector)!.getBoundingClientRect();
      return { x: r.x, right: r.right, y: r.y, width: r.width, height: r.height };
    };
    const header = rect(".note-group-header"), list = rect(".note-tabs-list");
    const add = rect(".note-tab-create"), toggle = rect(".note-inspector-toggle");
    return { header, list, add, toggle, gap: add.x - list.right,
      inset: header.right - toggle.right,
      scroll: el.querySelector(".note-tabs-list")!.scrollWidth > list.width,
      documentOverflow: document.documentElement.scrollWidth > innerWidth,
      visibleAtToggle: el.querySelector(".note-inspector-toggle")!.contains(document.elementFromPoint(toggle.x + toggle.width / 2, toggle.y + toggle.height / 2)) };
  });
}
async function assertGeometry(group: Locator) {
  const g = await geometry(group);
  expect(g.gap).toBeGreaterThanOrEqual(0);
  expect(g.gap).toBeLessThanOrEqual(5);
  expect(g.inset).toBeGreaterThanOrEqual(10);
  expect(g.inset).toBeLessThanOrEqual(16);
  expect(g.add.right).toBeLessThanOrEqual(g.toggle.x);
  expect(g.visibleAtToggle).toBe(true);
  expect(g.documentOverflow).toBe(false);
}

test("many tabs keep create and inspector reachable while direct move preserves both pages", async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { fixture: fixture(), locale: "en", rawActions: [], localStorage: { "wenlan-theme": "dark" } });
  await page.goto("/");
  for (let i = 1; i <= 18; i++) await openWikiNote(page, `Layout page ${i}`);
  await expect(frame(page).getByRole("tab", { name: "Layout page 18", exact: true })).toHaveAttribute("aria-selected", "true");
  await info.attach("overflow-geometry", { body: JSON.stringify(await geometry(frame(page))), contentType: "application/json" });
  await page.screenshot({ path: info.outputPath("many-tabs.png") });
  await assertGeometry(frame(page));
  expect((await geometry(frame(page))).scroll).toBe(true);
  const editor = frame(page).getByRole("textbox", { name: "Page editor", exact: true });
  await editor.evaluate(el => { const view = (el as any).cmTile.root.view; view.dispatch({ selection: { anchor: view.state.doc.length } }); view.focus(); });
  await page.keyboard.insertText("\nPreserved during direct move.");
  runtime.failNext("update_page", "save refused");
  await frame(page).getByRole("tab", { name: "Layout page 18", exact: true }).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to right group", exact: true }).click();
  await expect(frame(page).getByRole("alert")).toBeVisible();
  await expect(frame(page, "secondary")).toHaveCount(0);
  await frame(page).getByRole("button", { name: "Retry", exact: true }).click();
  await expect(frame(page).locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
  await frame(page).getByRole("tab", { name: "Layout page 18", exact: true }).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to right group", exact: true }).click();
  await expect(frame(page, "secondary").getByRole("tab", { name: "Layout page 18", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(frame(page, "secondary").getByRole("textbox", { name: "Page editor", exact: true })).toContainText("Preserved during direct move.");
  for (const width of [1440, 900, 375]) {
    await page.setViewportSize({ width, height: 900 });
    for (const id of ["primary", "secondary"]) await assertGeometry(frame(page, id));
    await page.screenshot({ path: info.outputPath(`split-${width}.png`) });
  }
  await frame(page, "secondary").getByRole("tab", { name: "Layout page 18", exact: true }).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to central group", exact: true }).click();
  await expect(frame(page, "secondary")).toHaveCount(0);
  await expect(frame(page).getByRole("tab", { name: "Layout page 18", exact: true })).toHaveAttribute("aria-selected", "true");
  await frame(page).getByRole("tab", { name: "Layout page 18", exact: true }).focus();
  await page.keyboard.press("Home");
  await expect(frame(page).getByRole("tab", { name: "Layout page 1", exact: true })).toBeFocused();
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
