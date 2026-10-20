// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from '@playwright/test';
import { collectBrowserErrors, installTauriMock } from './tauriMock';
import { openWikiNote } from './helpers/wikiWorkspace';
import { openPrimaryDestination } from './helpers/primaryNavigation';

const key = 'wenlan-right-sidebar-v1:primary';
const handle = (page: Page) => page.getByTestId('right-sidebar-resize-handle');
const pane = (page: Page) => page.locator('.page-info-drawer-hosted, .page-info-drawer-group-overlay-panel');
async function stored(page: Page) { return page.evaluate(k => localStorage.getItem(k), key); }
async function start(page: Page, theme = 'light') {
  await page.setViewportSize({ width: 1600, height: 900 });
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: 'en', rawActions: [], preserveLocalStorage: true, localStorage: { 'wenlan-theme': theme } });
  await page.goto('/'); await openWikiNote(page, 'Fixture architecture');
  await page.getByRole('button', { name: 'Open page sidebar', exact: true }).click();
  await expect(handle(page)).toBeVisible();
  return { errors, controller };
}
async function begin(page: Page) {
  const box = (await handle(page).boundingBox())!;
  const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await page.mouse.move(point.x, point.y); await page.mouse.down(); return point;
}
async function resize(page: Page, width: number) {
  const before = (await pane(page).boundingBox())!.width, p = await begin(page);
  await page.mouse.move(p.x + before - width, p.y, { steps: 8 }); await page.mouse.up();
}
async function expectWidth(page: Page, width: number) {
  await expect.poll(async () => (await pane(page).boundingBox())?.width).toBe(width);
}

test('mirrored drag previews without saving and restores width across tabs close and reload', async ({ page }) => {
  const { errors, controller } = await start(page);
  const editor = page.getByRole('textbox', { name: 'Page editor', exact: true });
  await expect(editor).toBeVisible(); const body = await editor.innerText();
  await editor.evaluate(el => el.setAttribute('data-resize-original', 'true'));
  const before = (await pane(page).boundingBox())!.width, initial = await stored(page), p = await begin(page);
  await page.mouse.move(p.x - 40, p.y, { steps: 8 }); await expectWidth(page, before + 40); expect(await stored(page)).toBe(initial);
  await page.mouse.up(); await expect.poll(() => stored(page)).not.toBe(initial);
  await expect(editor).toHaveAttribute('data-resize-original', 'true'); expect(await editor.innerText()).toBe(body);
  const saved = await stored(page);
  await page.getByRole('tab', { name: 'Info', exact: true }).click(); await expectWidth(page, before + 40);
  await page.getByRole('tab', { name: 'Mind map', exact: true }).click(); await expectWidth(page, before + 40);
  await expect(page.locator('.page-canvas')).toBeVisible();
  await page.getByRole('tab', { name: 'Info', exact: true }).click(); await expectWidth(page, before + 40);
  await page.getByRole('button', { name: 'Close page sidebar', exact: true }).click(); await expect(handle(page)).toHaveCount(0);
  await page.getByRole('button', { name: 'Open page sidebar', exact: true }).click(); await expectWidth(page, before + 40);
  await page.reload(); await openWikiNote(page, 'Fixture architecture');
  await page.getByRole('button', { name: 'Open page sidebar', exact: true }).click(); await expectWidth(page, before + 40);
  expect(await stored(page)).toBe(saved);
  expect(controller.calls().filter(c => ['update_page','put_page_map_layout','patch_page_map_node'].includes(c.command))).toHaveLength(0);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test('cancel blur lost capture and viewport changes restore geometry without closing or saving', async ({ page }) => {
  await start(page); const before = (await pane(page).boundingBox())!.width, initial = await stored(page);
  for (const reason of ['escape','cancel','blur','capture']) {
    const p = await begin(page); await page.mouse.move(p.x - 40, p.y); await expectWidth(page, before + 40);
    if (reason === 'escape') await page.keyboard.press('Escape');
    else if (reason === 'blur') await page.evaluate(() => window.dispatchEvent(new Event('blur')));
    else await handle(page).dispatchEvent(reason === 'cancel' ? 'pointercancel' : 'lostpointercapture', { pointerId: 1, pointerType: 'mouse', isPrimary: true });
    await page.mouse.up(); await expectWidth(page, before); expect(await stored(page)).toBe(initial);
  }
  const p = await begin(page); await page.mouse.move(p.x - 40, p.y); await page.setViewportSize({ width: 1099, height: 900 }); await page.mouse.up();
  await expect(page.locator('[data-note-group-id="primary"] .page-info-drawer-group-overlay')).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Page views', exact: true })).toBeVisible();
  expect(await stored(page)).toBe(initial); await page.setViewportSize({ width: 1600, height: 900 }); await expectWidth(page, before);
  expect(await page.evaluate(() => document.body.className)).not.toContain('resize-active');
});

test('keyboard mirrors direction and closing restores last width while viewport caps preserve preference', async ({ page }) => {
  await start(page); await handle(page).focus(); await page.keyboard.press('End'); await expectWidth(page, 600);
  await page.keyboard.press('ArrowRight'); await expectWidth(page, 580); await page.keyboard.press('ArrowLeft'); await expectWidth(page, 600);
  const saved = await stored(page); await page.setViewportSize({ width: 1208, height: 900 });
  // The labelled rail folds to icons here, so the cap leaves the note 360px beside the list.
  await expect(page.locator('.memory-shell')).toHaveAttribute('data-sidebar-layout', 'icons');
  await expect.poll(async () => (await pane(page).boundingBox())!.width).toBeLessThanOrEqual(1208 - 64 - 248 - 360);
  expect(await stored(page)).toBe(saved);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1600, height: 900 }); await expectWidth(page, 600);
  await handle(page).focus(); await page.keyboard.press('Home'); await expect(handle(page)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Open page sidebar', exact: true })).toBeFocused();
  await page.keyboard.press('Enter'); await expectWidth(page, 600);
  await resize(page, 30); await expect(handle(page)).toHaveCount(0); expect(await stored(page)).toBe(saved);
  await page.getByRole('button', { name: 'Open page sidebar', exact: true }).click(); await expectWidth(page, 600);
});

for (const theme of ['light','dark']) test(`${theme} grip mirrors left hover and disappears on release without leaving`, async ({ page }, info) => {
  await start(page, theme); const opacity = () => handle(page).evaluate(el => getComputedStyle(el, '::after').opacity);
  await page.mouse.move(700, 600); await expect.poll(opacity).toBe('0');
  const p = await begin(page); await expect.poll(opacity).toBe('1'); await page.mouse.move(p.x - 32, p.y, { steps: 6 });
  await page.screenshot({ path: info.outputPath('during-drag.png') });
  await page.mouse.up(); await expect.poll(opacity).toBe('0'); expect(await handle(page).evaluate(el => el.matches(':hover'))).toBe(true);
  await page.screenshot({ path: info.outputPath('after-release.png') });
  await page.mouse.move(700, 600); const box=(await handle(page).boundingBox())!; await page.mouse.move(box.x + 4, p.y); await expect.poll(opacity).toBe('1');
});

test('Graph keeps its own docked width and narrow panes keep modal focus controls', async ({ page }) => {
  const { errors } = await start(page); await resize(page, 500);
  const noteWidth = await stored(page);
  await page.getByRole('button', { name: 'Close page sidebar', exact: true }).click();
  await page.evaluate(() => localStorage.setItem('wenlan-right-sidebar-v1', JSON.stringify({ version: 1, width: 420 })));
  await openPrimaryDestination(page, 'Graph'); await page.getByRole('combobox', { name: 'Filter nodes' }).fill('Fixture architecture');
  await page.getByRole('option', { name: /Fixture architecture/ }).click(); await expectWidth(page, 420);
  await resize(page, 400); await expectWidth(page, 400);
  const saved = await page.evaluate(() => localStorage.getItem('wenlan-right-sidebar-v1'));
  expect(await stored(page)).toBe(noteWidth);
  await page.setViewportSize({ width: 375, height: 812 }); await expect(handle(page)).toHaveCount(0);
  const modal=page.getByRole('dialog'); await expect(modal).toHaveAttribute('aria-modal','true');
  await page.keyboard.press('Tab'); await expect.poll(() => modal.evaluate(el => el.contains(document.activeElement))).toBe(true);
  await page.keyboard.press('Escape'); await expect(modal).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem('wenlan-right-sidebar-v1'))).toBe(saved);
  expect(await stored(page)).toBe(noteWidth);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
