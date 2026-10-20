// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from '@playwright/test';
import { collectBrowserErrors, installTauriMock } from './tauriMock';
import { openWikiNote } from './helpers/wikiWorkspace';

const key = 'wenlan-navigation-v1';
const sidebar = (page: Page) => page.locator('.memory-sidebar, .settings-sidebar').first();
const seam = (page: Page) => page.getByRole('separator', { name: 'Resize sidebar', exact: true });
async function start(page: Page) {
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: 'en', rawActions: [], preserveLocalStorage: true,
    localStorage: { 'wenlan-theme': 'light', [key]: JSON.stringify({ version: 1, visible: ['pages','spaces','graph','sources'], sidebar: { visible: true, mode: 'labels' } }) } });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Open a page', exact: true })).toBeVisible();
  await expect(seam(page)).toBeVisible();
}
async function stored(page: Page) { return page.evaluate(k => JSON.parse(localStorage.getItem(k)!), key); }
async function beginDrag(page: Page) {
  const box = await seam(page).boundingBox();
  if (!box) throw new Error('Missing sidebar separator');
  const x = box.x + box.width / 2;
  const y = box.y + Math.min(180, box.height / 2);
  await page.mouse.move(x, y); await page.mouse.down();
  return { x, y };
}
async function dragToWidth(page: Page, width: number) {
  const before = (await sidebar(page).boundingBox())!.width;
  const { x, y } = await beginDrag(page);
  await page.mouse.move(x + width - before, y, { steps: 10 });
  await page.mouse.up();
}
async function widthMatches(page: Page, width: number) {
  await expect(sidebar(page)).toHaveCSS('width', `${width}px`);
  await expect(page.locator('.workspace-header-sidebar-backdrop')).toHaveCSS('width', `${width === 64 ? 0 : width}px`);
  if (width === 64 || width >= 200) {
    const expectedDisplay = width === 64 ? 'none' : 'block';
    await expect.poll(() => page.locator('.memory-shell').evaluate((element) => getComputedStyle(element, '::before').display)).toBe(expectedDisplay);
    if (width >= 200) await expect.poll(() => page.locator('.memory-shell').evaluate((element) => getComputedStyle(element, '::before').top)).toBe('0px');
  }
}

test('drag resize is shared by Wiki Spaces Graph Settings and persists only after release', async ({ page }) => {
  const errors = collectBrowserErrors(page); await start(page);
  const initial = await stored(page); const { x, y } = await beginDrag(page);
  await page.mouse.move(x + 64, y, { steps: 8 });
  await widthMatches(page, 304); expect(await stored(page)).toEqual(initial);
  await page.mouse.up();
  await expect.poll(async () => (await stored(page)).sidebar.width).toBe(304);
  for (const name of ['Spaces', 'Graph', 'Sources', 'Wiki']) {
    await page.locator('.notes-rail-nav').getByRole('button', { name, exact: true }).click();
    await widthMatches(page, 304); await expect(seam(page)).toBeVisible();
  }
  await page.locator('.memory-sidebar').getByRole('button', { name: 'Account menu' }).click();
  await page.getByRole('menuitem', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'General', exact: true })).toBeVisible();
  await widthMatches(page, 304);
  await dragToWidth(page, 280); await widthMatches(page, 280);
  await page.locator('[data-sidebar-toggle]').click(); await widthMatches(page, 0);
  await page.locator('[data-sidebar-toggle]').click(); await widthMatches(page, 280);
  await page.reload(); await widthMatches(page, 280);
  expect((await stored(page)).visible).toEqual(['pages','spaces','graph','sources']);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test('one drag can pass through icons to hidden and toggle restores expanded width', async ({ page }) => {
  await start(page); await dragToWidth(page, 300);
  const before = await stored(page); const { x, y } = await beginDrag(page);
  await page.mouse.move(x - 180, y, { steps: 10 });
  await expect(page.locator('.memory-shell')).toHaveAttribute('data-sidebar-layout', 'icons'); await widthMatches(page, 64);
  expect(await stored(page)).toEqual(before);
  await page.mouse.move(x - 275, y, { steps: 10 });
  await expect(page.locator('.memory-shell')).toHaveAttribute('data-sidebar-layout', 'zero');
  await page.mouse.up(); await expect(seam(page)).toBeHidden();
  await expect(page.locator('[data-sidebar-toggle]')).toBeFocused();
  await page.locator('[data-sidebar-toggle]').press('Enter'); await widthMatches(page, 300);
  await dragToWidth(page, 120); await widthMatches(page, 64);
  await page.reload(); await widthMatches(page, 64);
  await dragToWidth(page, 240); await widthMatches(page, 240);
});

test('Escape and pointer cancellation restore geometry and do not persist partial drags', async ({ page }) => {
  await start(page); const initial = await stored(page);
  let point = await beginDrag(page); await page.mouse.move(point.x + 100, point.y);
  await widthMatches(page, 340); await page.keyboard.press('Escape'); await page.mouse.up();
  await widthMatches(page, 240); expect(await stored(page)).toEqual(initial);
  point = await beginDrag(page); await page.mouse.move(point.x - 120, point.y);
  await widthMatches(page, 64);
  await seam(page).dispatchEvent('pointercancel', { pointerId: 1, pointerType: 'mouse', isPrimary: true });
  await page.mouse.up(); await widthMatches(page, 240); expect(await stored(page)).toEqual(initial);
  point = await beginDrag(page); await page.mouse.move(point.x + 80, point.y);
  await widthMatches(page, 320); await page.setViewportSize({ width: 375, height: 812 });
  await page.mouse.up(); await expect(seam(page)).toBeHidden(); expect(await stored(page)).toEqual(initial);
  await page.setViewportSize({ width: 1280, height: 900 }); await widthMatches(page, 240);
});

test('keyboard resize reaches icon and hidden states; narrow drawer retains desktop preference', async ({ page }) => {
  await start(page); await seam(page).focus(); await page.keyboard.press('End'); await widthMatches(page, 360);
  await page.keyboard.press('ArrowLeft'); await widthMatches(page, 340);
  await page.keyboard.press('Home'); await widthMatches(page, 0);
  await expect(page.locator('[data-sidebar-toggle]')).toBeFocused();
  await page.keyboard.press('Enter'); await widthMatches(page, 340);
  const desktop = await stored(page);
  await page.setViewportSize({ width: 375, height: 812 });
  await expect(seam(page)).toBeHidden(); await expect(sidebar(page)).toBeHidden();
  await page.locator('[data-sidebar-toggle]').press('Enter'); await expect(sidebar(page)).toBeVisible();
  await expect(sidebar(page)).toHaveCSS('width', '240px');
  await page.keyboard.press('Escape'); await expect(sidebar(page)).toBeHidden();
  expect(await stored(page)).toEqual(desktop);
  await page.setViewportSize({ width: 1280, height: 900 }); await widthMatches(page, 340);
});

test('resizing during page editing preserves the editor and content', async ({ page }) => {
  await start(page);
  await openWikiNote(page, "Fixture architecture");
  const editor = page.locator('.cm-content[contenteditable=true]');
  await expect(editor).toContainText('Deterministic content');
  const body = await editor.innerText();
  await editor.evaluate(el => { el.setAttribute('data-resize-original-editor', 'true'); });
  const point = await beginDrag(page); await page.mouse.move(point.x + 80, point.y);
  await page.keyboard.press('Escape'); await page.mouse.up();
  await expect(editor).toHaveAttribute('data-resize-original-editor', 'true'); await widthMatches(page, 240);
  await dragToWidth(page, 320); await dragToWidth(page, 120);
  await expect(editor).toHaveAttribute('data-resize-original-editor', 'true');
  expect(await editor.innerText()).toBe(body);
  await seam(page).focus(); await page.keyboard.press('Home');
  await expect(editor).toBeVisible(); expect(await editor.innerText()).toBe(body);
  await page.locator('[data-sidebar-toggle]').click(); await widthMatches(page, 64);
});

for (const theme of ['light', 'dark']) test(`${theme} grip appears on hover and hides on release without leaving the seam`, async ({ page }, info) => {
  await start(page);
  await page.evaluate(value => document.documentElement.setAttribute('data-theme', value), theme);
  const handle = seam(page);
  const opacity = () => handle.evaluate(el => getComputedStyle(el, '::after').opacity);
  await page.mouse.move(600, 400);
  await expect.poll(opacity).toBe('0');
  const box = (await handle.boundingBox())!;
  const x = box.x + box.width / 2, y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await expect.poll(opacity).toBe('1');
  await page.screenshot({ path: info.outputPath('grip-hover.png'), clip: { x: 180, y: y - 55, width: 180, height: 110 } });
  await page.mouse.down();
  await page.mouse.move(x + 40, y, { steps: 4 });
  await expect.poll(opacity).toBe('1');
  await page.mouse.up();
  expect(await handle.evaluate(el => el.matches(':hover'))).toBe(true);
  await expect.poll(opacity).toBe('0');
  await page.screenshot({ path: info.outputPath('grip-release.png'), clip: { x: 180, y: y - 55, width: 180, height: 110 } });
  await page.mouse.move(600, y);
  await page.mouse.move(x + 40, y);
  await expect.poll(opacity).toBe('1');
  await page.mouse.move(600, y);
  await expect.poll(opacity).toBe('0');
  await page.keyboard.press('Tab');
  await handle.focus();
  await expect.poll(opacity).toBe('1');
});
