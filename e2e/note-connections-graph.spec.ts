// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect, type Page } from '@playwright/test';
import type { KnowledgeGraph } from '../src/lib/tauri';
import { installTauriMock, collectBrowserErrors } from './tauriMock';
import { openWikiNote } from './helpers/wikiWorkspace';
import { openPageTool } from './helpers/pageTools';
import { openPrimaryDestination } from './helpers/primaryNavigation';
import { openCanvas, box } from './helpers/pageCanvas';

const title = 'Fixture architecture';
async function boot(page: Page, width = 1280, mode = 'normal') {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width, height: 900 });
  const controller = await installTauriMock(page, { locale: 'en', rawActions: [], localStorage: {
    'atlas.layers': JSON.stringify({ entity: true, page: true, memory: false }),
  } });
  if (mode !== 'normal') await page.addInitScript(({ mode }) => {
    const api = window.__TAURI_INTERNALS__!; const original = api.invoke;
    api.invoke = async (command, args) => {
      const out = await original(command, args);
      if (command === 'get_knowledge_graph_cmd') {
        const graph = out as KnowledgeGraph;
        if (mode === 'empty') graph.page_links = graph.page_links.filter(link => link.from.id !== 'page-architecture' && link.to.id !== 'page-architecture');
        if (mode === 'missing') graph.pages = graph.pages.filter(p => p.id !== 'page-architecture');
      }
      return out;
    };
  }, { mode });
  await page.goto('/'); await openWikiNote(page, title);
  await expect(page.getByRole('button', { name: 'Info', exact: true })).toBeVisible();
  await openPageTool(page, 'Page info');
  return { errors, controller };
}

for (const width of [1280, 375]) test(`whole graph handoff and roundtrip ${width}`, async ({ page }, info) => {
  const { errors } = await boot(page, width);
  await page.getByRole('button', { name: 'Open in graph', exact: true }).press('Enter');
  await expect(page.locator('.atlas-count-line')).toContainText('7 pages');
  await expect(page.getByRole('button', { name: 'Show full graph', exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem('atlas.layers'))).toBe(JSON.stringify({ entity: true, page: true, memory: false }));
  await page.screenshot({ path: info.outputPath('01-focused.png') });
  const filter = page.getByRole('combobox');
  await filter.fill('Weekly');
  await expect(page.getByRole('option', { name: 'Weekly fixture recap', exact: true })).toBeVisible();
  await filter.fill('Fixture architecture'); await filter.press('Enter');
  await expect(page.locator('.atlas-open-detail')).toBeVisible();
  if (width >= 1100) await expect.poll(() => page.locator('canvas.sigma-nodes').evaluate((el: HTMLCanvasElement) => Math.abs(el.width / devicePixelRatio - el.parentElement!.clientWidth))).toBeLessThanOrEqual(1);
  await page.locator('.atlas-open-detail').click();
  await expect(page.getByRole('textbox', { name: 'Page editor', exact: true })).toBeVisible();
  await page.locator('.workspace-history-button').first().click();
  await expect(page.locator('.atlas-count-line')).toContainText('7 pages');
  await page.getByRole('button', { name: 'Fit entire graph', exact: true }).click();
  await page.screenshot({ path: info.outputPath('02-zoomed-out.png') });
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test('global Graph navigation remains the same destination without a page focus', async ({ page }) => {
  const { errors } = await boot(page);
  await page.getByRole('button', { name: 'Open in graph', exact: true }).click();
  await expect(page.locator('.atlas-count-line')).toContainText('7 pages');
  await openPrimaryDestination(page, 'Graph');
  await expect(page.getByTestId('atlas-view')).toBeVisible();
  await expect(page.locator('.atlas-count-line')).toContainText('7 pages');
  expect(errors.pageErrors).toEqual([]);
});

for (const mode of ['empty', 'missing']) test(`safe ${mode} page graph`, async ({ page }, info) => {
  const { errors } = await boot(page, 1280, mode);
  await page.getByRole('button', { name: 'Open in graph', exact: true }).click();
  await expect(page.getByTestId('atlas-view')).toBeVisible();
  await expect(page.locator('.atlas-count-line')).toContainText(mode === 'missing' ? '6 pages' : '7 pages');
  const filter = page.getByRole('combobox');
  await filter.fill('Weekly');
  await expect(page.getByRole('option', { name: 'Weekly fixture recap', exact: true })).toBeVisible();
  await filter.fill('Fixture architecture');
  await expect(page.getByRole('option')).toHaveCount(mode === 'empty' ? 1 : 0);
  await filter.fill(''); await page.screenshot({ path: info.outputPath(mode + '.png') });
  expect(errors.pageErrors).toEqual([]);
});

test('failed page save blocks graph navigation and preserves editor/sidebar', async ({ page }) => {
  const { errors, controller } = await boot(page);
  await page.getByRole('button', { name: 'Info', exact: true }).click();
  controller.failNext('update_page', 'Synthetic save failure', 20);
  const editor = page.getByRole('textbox', { name: 'Page editor', exact: true });
  await editor.click(); await page.keyboard.press('End'); await page.keyboard.type(' protected edit');
  await page.getByRole('button', { name: 'Info', exact: true }).click();
  await page.getByRole('button', { name: 'Open in graph', exact: true }).click();
  await expect(page.getByTestId('atlas-view')).toHaveCount(0);
  await expect(editor).toContainText('protected edit');
  await expect(page.getByRole('button', { name: 'Open in graph', exact: true })).toBeVisible();
  expect(errors.pageErrors).toEqual([]);
});

/** Where the handed-off page sits on the drawn map, read off sigma (dev handle). */
async function drawnMap(page: Page) {
  return page.evaluate(() => {
    const sigma = (window as unknown as { __ATLAS_SIGMA: { getGraph(): { nodes(): string[]; getNodeAttributes(id: string): { x: number; y: number } }; graphToViewport(p: { x: number; y: number }): { x: number; y: number }; getDimensions(): { width: number; height: number }; getCamera(): { ratio: number } } }).__ATLAS_SIGMA;
    const graph = sigma.getGraph(); const { width, height } = sigma.getDimensions();
    const at = (id: string) => sigma.graphToViewport(graph.getNodeAttributes(id));
    const onScreen = graph.nodes().filter((id) => { const p = at(id); return p.x >= 0 && p.x <= width && p.y >= 0 && p.y <= height; }).length;
    return { width, height, onScreen, page: at('page:page-architecture'), ratio: sigma.getCamera().ratio };
  });
}

for (const width of [1280, 375]) test(`the big map is drawn and centred on the handed-off page at ${width}`, async ({ page }, info) => {
  const { errors } = await boot(page, width);
  await page.getByRole('button', { name: 'Open in graph', exact: true }).click();
  await expect(page.locator('.atlas-count-line')).toContainText('7 pages');
  await page.waitForTimeout(500);
  const map = await drawnMap(page);
  expect(map.onScreen).toBeGreaterThan(1);
  expect(Math.abs(map.page.x - map.width / 2)).toBeLessThan(3);
  expect(Math.abs(map.page.y - map.height / 2)).toBeLessThan(3);
  await page.screenshot({ path: info.outputPath('handoff.png') });
  expect(errors.pageErrors).toEqual([]);
});

test('Show on the big map from the page mind map draws the map on the page and keeps Reset view working', async ({ page }, info) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: 'en', rawActions: [] });
  await openCanvas(page);
  await box(page, 'Fixture architecture').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Show on the big map', exact: true }).click();
  await expect(page.getByTestId('atlas-view')).toBeVisible();
  await expect(page.locator('.atlas-open-detail')).toBeVisible();
  await page.waitForTimeout(500);
  const map = await drawnMap(page);
  expect(map.onScreen).toBeGreaterThan(1);
  expect(Math.abs(map.page.x - map.width / 2)).toBeLessThan(3);
  expect(Math.abs(map.page.y - map.height / 2)).toBeLessThan(3);
  await page.screenshot({ path: info.outputPath('mind-map-handoff.png') });
  await page.getByRole('button', { name: 'Reset view', exact: true }).click();
  await page.waitForTimeout(300);
  expect((await drawnMap(page)).onScreen).toBeGreaterThan(1);
  expect(errors.pageErrors).toEqual([]);
});
