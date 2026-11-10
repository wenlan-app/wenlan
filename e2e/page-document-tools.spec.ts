// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openPageTool } from "./helpers/pageTools";
import { openWikiNote } from "./helpers/wikiWorkspace";

const copies = [
  { locale: "en", open: "Open Fixture architecture", editor: "Page editor", actions: "Page actions", info: "Page info", map: "Mind map", note: "Page", close: "Close", pane: "Page views", toggle: "Info", untoggle: "Info" },
] as const;

async function expectDocumentToolsAligned(page: Page, actionsName: string, toggleName: string): Promise<void> {
  const actions = page.getByRole("button", { name: actionsName, exact: true });
  await expect(actions).toBeInViewport();
  // Page actions sit in the top bar, just left of the page sidebar toggle,
  // so the title line holds only the title.
  await expect(page.locator(".note-group-document-tools.wiki-shared-document-tools").getByRole("button", { name: actionsName, exact: true })).toBeVisible();
  const toggle = page.getByRole("button", { name: toggleName, exact: true });
  // Closing a pane reflows the header. Read both rectangles in one frame,
  // then poll the unchanged geometry contract until that transition settles.
  await expect(async () => {
    const geometry = await page.evaluate(([action, context]) => {
      if (!action || !context) throw new Error("Document tools are unavailable");
      const a = action.getBoundingClientRect();
      const b = context.getBoundingClientRect();
      return {
        centerDifference: Math.abs(a.y + a.height / 2 - (b.y + b.height / 2)),
        actionRight: a.right,
        toggleLeft: b.left,
        viewportWidth: innerWidth,
      };
    }, [await actions.elementHandle(), await toggle.elementHandle()]);
    expect(geometry.centerDifference).toBeLessThanOrEqual(1);
    expect(geometry.actionRight).toBeLessThanOrEqual(geometry.toggleLeft + 1);
    expect(geometry.actionRight).toBeLessThanOrEqual(geometry.viewportWidth);
  }).toPass({ timeout: 10_000 });
}

test("narrow document tools align across platform font metrics", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  // The system font differs on macOS and Linux. Icon buttons must not inherit
  // a vertical offset from their wrapper's text baseline on either platform.
  for (const font of ["sans-serif", "serif"]) {
    await page.locator(".note-group-document-tools.wiki-shared-document-tools").evaluate((node, family) => {
      (node as HTMLElement).style.fontFamily = family;
    }, font);
    await expectDocumentToolsAligned(page, "Page actions", "Info");
  }
});

for (const copy of copies) for (const width of [1280, 375]) {
  test(`document actions and dedicated inspector: ${copy.locale} ${width}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale: copy.locale, rawActions: [] });
    await page.goto("/");
    await openWikiNote(page, "Fixture architecture");
    await expect(page.locator(".page-info-drawer")).toHaveCount(0);
    await expect(page.locator(".note-inspector-tabs")).toHaveCount(0);
    const infoButton = page.getByRole("button", { name: "Info", exact: true });
    const mapButton = page.getByRole("button", { name: copy.map, exact: true });
    await expect(infoButton).toBeVisible();
    await expect(mapButton).toBeVisible();
    await mapButton.click();
    await expect(page.getByRole("region", { name: "Canvas for Fixture architecture", exact: true })).toBeVisible();
    await expect(page.getByRole("complementary", { name: copy.pane, exact: true })).toBeVisible();
    await mapButton.click();
    await expect(page.locator(".page-info-drawer")).toHaveCount(0);
    await expect(page.locator(".note-inspector-tabs")).toHaveCount(0);
    await infoButton.click();
    await expect(infoButton).toHaveAttribute("aria-pressed", "true");
    await infoButton.click();
    await expect(page.locator(".page-info-drawer")).toHaveCount(0);
    const editor = page.getByRole("textbox", { name: copy.editor, exact: true });
    // Note-group inspectors remain non-modal at every viewport width.
    const role = "complementary";
    await expect(editor).toBeEditable();
    for (const name of [copy.note, copy.map, copy.info]) {
      await expect(page.locator(".page-detail").getByRole("button", { name, exact: true })).toHaveCount(0);
    }
    await expect(page.locator(".page-detail-top-row, .page-detail-view-controls, .page-detail-canvas-toggle")).toHaveCount(0);
    await expect(mapButton).toBeVisible();
    await expectDocumentToolsAligned(page, copy.actions, copy.toggle);
    await page.screenshot({ path: testInfo.outputPath("note-title-tools.png") });

    await openPageTool(page, copy.info, copy.actions);
    const info = page.getByRole(role, { name: copy.pane, exact: true });
    await expect(info).toBeVisible();
    await expect(page.getByRole("button", { name: copy.untoggle, exact: true })).toHaveAttribute("aria-expanded", "true");
    await expect(infoButton).toHaveAttribute("aria-pressed", "true");
    const entry = mapButton;
    await expect(entry).toHaveAttribute("aria-pressed", "false");
    await expect(entry).toBeInViewport();
    const entryBox = (await entry.boundingBox())!;
    expect(entryBox.x).toBeGreaterThanOrEqual(0);
    expect(entryBox.x + entryBox.width).toBeLessThanOrEqual(width);
    await expect(editor).toBeEditable();
    await infoButton.press("Enter");
    await expect(info).toHaveCount(0);
    await expect(page.getByRole("button", {name:copy.toggle, exact:true})).toBeFocused();
    await expect(editor).toBeEditable();
    expect(controller.calls().filter((call) => call.command === "update_page")).toHaveLength(0);

    const draft = "# Fixture architecture\n\nSaved before the map pane opens.";
    await editor.fill(draft);
    await openPageTool(page, copy.info, copy.actions);
    await entry.press("Enter");
    await expect(entry).toHaveAttribute("aria-pressed", "true");
    const map = page.getByRole(role, { name: copy.pane, exact: true });
    await expect(map).toBeVisible();
    await expect(map.locator(".page-canvas")).toBeVisible();
    await expect(map.locator(".page-canvas-surface")).toBeFocused();
    await expect(editor).toHaveCount(0);
    const reading = page.getByTestId("page-document-reading");
    await expect(reading).toContainText("Saved before the map pane opens.");
    await expect(reading.locator('[contenteditable="true"], textarea')).toHaveCount(0);
    const calls = controller.calls();
    const writeIndex = calls.findIndex((call) => call.command === "update_page"
      && typeof call.args === "object" && call.args !== null
      && "content" in call.args && call.args.content === draft);
    const mapIndex = calls.findIndex((call, index) => index > writeIndex && call.command === "get_page_map");
    expect(writeIndex).toBeGreaterThanOrEqual(0);
    expect(mapIndex).toBeGreaterThan(writeIndex);
    expect(calls[writeIndex].args).toMatchObject({ content: draft });
    const saved = await page.evaluate(async () =>
      window.__wenlanTauriInvoke("get_page", { id: "page-architecture" }),
    );
    expect(saved).toMatchObject({ content: draft });
    const mapBox = (await map.boundingBox())!;
    if (width >= 1100) {
      const noteBox = (await page.locator(".page-detail-document").boundingBox())!;
      expect(mapBox.width).toBeGreaterThanOrEqual(320); // Current note-group inspector default width.
      expect(noteBox.width).toBeGreaterThanOrEqual(240);
      expect(mapBox.x).toBeGreaterThanOrEqual(noteBox.x + noteBox.width - 1);
    } else {
      const frame = page.locator(".wiki-primary-note-group .note-group-frame");
      const frameBox = (await frame.boundingBox())!;
      const shellBox = (await page.locator(".memory-shell").boundingBox())!;
      const groupHeaderBox = (await frame.locator(".note-group-header").boundingBox())!;
      const noteContentBox = (await frame.locator(".note-group-content").boundingBox())!;
      expect(mapBox.x).toBeGreaterThanOrEqual(frameBox.x);
      expect(mapBox.x + mapBox.width).toBeLessThanOrEqual(shellBox.x + shellBox.width + 1);
      expect(mapBox.y).toBeGreaterThanOrEqual(groupHeaderBox.y + groupHeaderBox.height - 1);
      expect(mapBox.y + mapBox.height).toBeLessThanOrEqual(frameBox.y + frameBox.height + 1);
      expect(noteContentBox.width).toBeGreaterThanOrEqual(await page.evaluate(() => matchMedia("(pointer: coarse)").matches ? 228 : 204));
    }
    await page.screenshot({ path: testInfo.outputPath("map-pane-and-note.png") });
    await entry.press("Enter");
    await expect(map).toHaveCount(0);
    await expect(editor).toBeEditable();
    await expect(editor).toContainText("Saved before the map pane opens.");
    await expect(page.getByRole("button", { name: copy.toggle, exact: true })).toHaveAttribute("aria-expanded", "false");
    await expect(editor).toBeFocused();
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    expect(errors.pageErrors).toEqual([]);
  });
}

test("a failed flush keeps the newest draft editable and never opens the map writer", async ({ page }) => {
  const controller = await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    failures: [{ command: "update_page", message: "save unavailable", times: 99 }],
  });
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeEditable();
  const draft = "# Fixture architecture\n\nKeep the unsaved draft after the failed map transition.";
  await editor.fill(draft);
  await openPageTool(page, "Mind map");
  await expect(page.locator(".page-editor-notice[role=alert]")).toBeVisible();
  await expect(editor).toBeEditable();
  await expect(editor).toContainText("Keep the unsaved draft after the failed map transition.");
  await expect(page.locator(".page-canvas")).toHaveCount(0);
  expect(controller.calls().filter((call) => call.command === "get_page_map")).toHaveLength(0);
  const saved = await page.evaluate(async () =>
    window.__wenlanTauriInvoke("get_page", { id: "page-architecture" }),
  );
  expect(saved).not.toMatchObject({ content: draft });
});

test("the page and its action menu do not send shortcuts to the open map", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const controller = await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  await openPageTool(page, "Mind map");
  const map = page.getByRole("complementary", { name: "Page views", exact: true });
  const node = map.locator(".react-flow__node").filter({ hasText: "Storage layer" }).first();
  await node.click();
  await expect(map.locator(".react-flow__node.selected")).toHaveCount(1);
  const writes = () => controller.calls().filter((call) =>
    ["put_page_map_layout", "patch_page_map_node", "delete_page_map_node", "create_page_map_node"].includes(call.command),
  ).length;
  expect(writes()).toBe(0); // Opening and selecting an automatic layout must not persist it.
  const before = writes();
  const actions = page.getByRole("button", { name: "Page actions", exact: true });
  await actions.click();
  const menu = page.getByRole("menu", { name: "Page actions", exact: true });
  // Map-open state disables stored-page actions. The empty menu still owns
  // focus so keyboard shortcuts and Escape cannot fall through to the map.
  await expect(menu.locator('[role="menuitem"]:not(:disabled)')).toHaveCount(0);
  await expect(menu).toHaveAttribute("tabindex", "-1");
  await expect(menu).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu).toBeFocused();
  await page.keyboard.press("Delete");
  await page.keyboard.press("ControlOrMeta+a");
  await expect(node).toBeVisible();
  await expect(map.locator(".react-flow__node.selected")).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(actions).toBeFocused();
  await expect(map).toBeVisible();
  await expect(map.locator(".react-flow__node.selected")).toHaveCount(1);
  await page.getByTestId("page-document-reading").click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  await expect(node).toBeVisible();
  await expect(map.locator(".react-flow__node.selected")).toHaveCount(1);
  await page.waitForTimeout(700); // Covers the map layout write debounce.
  expect(writes()).toBe(before);

  // Pointer reentry restores map keyboard ownership after using the note.
  await node.click();
  await page.keyboard.press("ArrowRight");
  await expect.poll(writes).toBeGreaterThan(before);
});

test("docked tools resize the complete workspace and keep search usable across the pane breakpoint", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await openPageTool(page, "Mind map");
  const pane = page.getByRole("complementary", { name: "Page views", exact: true });
  const header = page.locator(".memory-workspace-header");
  const frame = page.locator(".wiki-primary-note-group .note-group-frame");
  const groupHeader = frame.locator(".note-group-header");
  const search = page.getByRole("dialog", { name: "Search", exact: true }).getByRole("textbox");
  await expect(pane).toBeVisible();
  const panelBox = (await pane.boundingBox())!;
  const frameBox = (await frame.boundingBox())!;
  const groupHeaderBox = (await groupHeader.boundingBox())!;
  const noteBox = (await page.locator(".page-detail-document").boundingBox())!;
  expect(panelBox.y).toBeGreaterThanOrEqual(groupHeaderBox.y + groupHeaderBox.height - 1);
  expect(panelBox.y + panelBox.height).toBeLessThanOrEqual(frameBox.y + frameBox.height + 1);
  expect(panelBox.x).toBeGreaterThanOrEqual(noteBox.x + noteBox.width - 1);
  expect(panelBox.x + panelBox.width).toBeLessThanOrEqual(await page.evaluate(() => innerWidth));
  expect(panelBox.width).toBeGreaterThanOrEqual(320); // Minimum useful docked inspector width.
  expect(noteBox.width).toBeGreaterThanOrEqual(240);
  await expect(search).toBeHidden();
  await page.locator(".notes-workspace-sidebar").getByRole("button", { name: "Search", exact: true }).click();
  await expect(search).toBeFocused();
  await search.press("Escape");
  await expect(search).toBeHidden();
  await expect(pane).toBeVisible();

  await page.getByRole("button", { name: "Info", exact: true }).focus();
  await page.setViewportSize({ width: 1099, height: 900 });
  const narrowPane = page.getByRole("complementary", { name: "Page views", exact: true });
  await expect(narrowPane).toBeVisible();
  expect((await header.boundingBox())!.width).toBe(1099);
  const narrowFrame = page.locator(".wiki-primary-note-group .note-group-frame");
  const narrowFrameBox = (await narrowFrame.boundingBox())!;
  const narrowPaneBox = (await narrowPane.boundingBox())!;
  expect(narrowPaneBox.x).toBeGreaterThanOrEqual(narrowFrameBox.x);
  expect(narrowPaneBox.x + narrowPaneBox.width).toBeLessThanOrEqual(await page.evaluate(() => innerWidth));
  await page.keyboard.press("Escape");
  await expect(narrowPane).toHaveCount(0);
  await expect(editor).toBeFocused();

  await page.setViewportSize({ width: 1280, height: 900 });
  await expect(header.getByRole("textbox")).toHaveCount(0);
  // Wide Wiki navigation is the one 240px sidebar, its page tree inside it
  // (UX-FIND-4); note-group tools occupy their own header alongside it.
  await expect.poll(async () => (await header.boundingBox())!.width).toBe(240);
  const restoredFrame = (await frame.boundingBox())!;
  expect(restoredFrame.x + restoredFrame.width).toBeLessThanOrEqual(1280);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});


test('many tabs leave the entire action group reachable with the main sidebar hidden',async({page},info)=>{
 await page.setViewportSize({width:1440,height:900});await installTauriMock(page,{locale:'en',rawActions:[],localStorage:{'wenlan-navigation-v1':JSON.stringify({version:2,visible:['pages','spaces','graph','sources'],sidebar:{visible:false,mode:'labels'}})}});await page.goto('/');
 for(const title of ['Fixture architecture','Why fixtures stay deterministic','Weekly fixture recap','CJK layout','History semantics','Independent research'])await openWikiNote(page,title);
 await expect(page.locator('.note-tabs-list [role=tab][aria-selected=true]')).toHaveText('Independent research');
 const plus=(await page.locator('.note-tab-create').boundingBox())!,actions=(await page.locator('.workspace-header-actions').boundingBox())!;
 await page.screenshot({path:info.outputPath('many-tabs.png')});expect(plus.x+plus.width).toBeLessThanOrEqual(actions.x+1);
 await expect(page.getByRole('button',{name:'Info',exact:true})).toBeInViewport();await page.getByRole('button',{name:'Info',exact:true}).click();await expect(page.getByRole('complementary',{name:'Page views',exact:true})).toBeVisible();
});
