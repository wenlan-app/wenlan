// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from '@playwright/test';
import { installTauriMock } from './tauriMock';
import { createSpacesNavigationFixture } from './fixtures/spacesNavigation';
import { createReviewDecisionFixture } from './fixtures/reviewDecisions';
import { showWikiFolders } from './helpers/wikiWorkspace';
import { openPrimaryDestination } from './helpers/primaryNavigation';

const body = '# Three references\n\nRead [城市散步](#concept:linked-page), [討論記憶](#memory:linked-memory), and the imported source [1].\n';
async function openReferences(page: Page) {
  const fixture = createReviewDecisionFixture('wiki-folders');
  const memory = { ...fixture.memories[0], source_id: 'linked-memory', title: '討論記憶', content: 'Keep the original evidence and its meaning.', summary: null };
  const source = { ...memory, source_id: 'fixture-folder::/tmp/已匯入的設計紀錄.md', title: '已匯入的設計紀錄', content: '# Design record\n\nA local imported source excerpt.', summary: null };
  const sourcePage = { ...fixture.pages[0], id: 'three-references', title: 'Three references', content: body,
    storage_path: 'Work/Three references.md', folder_path: 'Work', source_memory_ids: [source.source_id], citations: [{ occurrence: 1, marker: 1, source_kind: 'external_file' as const, locator: source.source_id, status: 'unverified' as const, score: 0.2, scope: 'sentence' as const }] };
  const target = { ...fixture.pages[0], id: 'linked-page', title: '城市散步', content: '# 城市散步\n\nThe linked page is readable.', summary: null };
  const controller = await installTauriMock(page, { locale: 'en', rawActions: [], fixture: { ...fixture, pages: [sourcePage, target], memories: [memory, source] } });
  await page.goto('/');
  await openPrimaryDestination(page, 'Wiki');
  const directory = await showWikiFolders(page);
  const panel = directory.locator('.notes-list-panel');
  await panel.getByRole('button', { name: 'Work', exact: true }).click();
  await panel.getByRole('button', { name: 'Open Three references', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'Page editor', exact: true });
  await expect(editor).toBeEditable();
  await page.mouse.move(1, 1);
  await editor.evaluate(el => {
    const view = (el as unknown as { cmTile: { root: { view: { state: { doc: { length: number } }; dispatch(spec: unknown): void; focus(): void } } } }).cmTile.root.view;
    view.dispatch({ selection: { anchor: view.state.doc.length } }); view.focus();
  });
  await expect(editor).not.toContainText('](#memory:');
  return { controller, editor };
}

test('page, memory and source previews share rendering without changing Markdown or opening originals', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const { controller, editor } = await openReferences(page);
  const pageLink = editor.getByRole('link', { name: '城市散步', exact: true });
  const memoryLink = editor.getByRole('link', { name: '討論記憶', exact: true });
  const sourceLink = editor.locator('a.reference-link--source');
  for (const link of [pageLink, memoryLink, sourceLink]) {
    await expect(link).toBeVisible();
    await expect(link).toHaveCSS('text-decoration-line', 'underline');
  }
  await expect(editor).not.toContainText('](#memory:');
  await expect(sourceLink).toHaveClass(/reference-link--unverified/);
  await pageLink.hover();
  await expect(page.getByRole('dialog', { name: '城市散步', exact: true })).toContainText('The linked page is readable.');
  await memoryLink.hover();
  await expect(page.getByRole('dialog', { name: '討論記憶', exact: true })).toContainText('Keep the original evidence');
  await sourceLink.hover();
  const sourcePreview = page.locator('[data-reference-preview]');
  await expect(sourcePreview).toContainText('A local imported source excerpt.');
  await expect(sourcePreview).toContainText(/unverified/i);
  expect(controller.calls().filter(call => ['open_file', 'plugin:shell|open', 'update_page'].includes(call.command))).toEqual([]);
  expect(controller.calls().filter(call => call.command === 'get_page_explicit_browse' && (call.args as { id?: string }).id === 'linked-page')).toEqual([]);
  const raw = await editor.evaluate(el => (el as unknown as { cmTile: { root: { view: { state: { doc: { toString(): string } } } } } }).cmTile.root.view.state.doc.toString());
  expect(raw).toBe(body);
  expect(errors).toEqual([]);
});

test('memory click and keyboard preview return use the shared navigation without corrupting the editor', async ({ page }) => {
  const { controller, editor } = await openReferences(page);
  const memoryLink = editor.getByRole('link', { name: '討論記憶', exact: true });
  await memoryLink.focus();
  await expect(page.getByRole('dialog', { name: '討論記憶', exact: true })).toBeVisible();
  await page.keyboard.press('Tab');
  await expect(page.locator('[data-reference-preview] button').last()).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-reference-preview]')).toHaveCount(0);
  await expect(memoryLink).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('.memory-detail-reading')).toContainText('Keep the original evidence');
  expect(controller.calls().filter(call => call.command === 'update_page')).toEqual([]);
});

test('a direct memory link flushes the dirty page through the existing save guard', async ({ page }) => {
  const { controller, editor } = await openReferences(page);
  await page.keyboard.insertText('Pending text.');
  await editor.getByRole('link', { name: '討論記憶', exact: true }).click();
  await expect(page.locator('.memory-detail-reading')).toContainText('Keep the original evidence');
  const writes = controller.calls().filter(call => call.command === 'update_page');
  expect(writes).toHaveLength(1);
  expect(writes[0].args).toMatchObject({ id: 'three-references', content: `${body}Pending text.`, expectedVersion: 1 });
  const index = controller.calls().indexOf(writes[0]);
  expect(controller.calls().slice(index + 1).some(call => ['get_page', 'get_page_explicit_browse'].includes(call.command)
    && (call.args as { id?: string }).id === 'three-references')).toBe(true);
});

test('links inside an imported source use the same preview and guarded in-app navigation', async ({ page }) => {
  const fixture = createSpacesNavigationFixture();
  const sourceText = 'Read [Linked page](#concept:page-architecture) and [Linked memory](#memory:memory-1).\n\n![tracking](https://tracker.invalid/never.png)';
  const external: string[] = [];
  await page.route('https://tracker.invalid/**', route => { external.push(route.request().url()); return route.abort(); });
  const controller = await installTauriMock(page, { locale: 'en', rawActions: [], fixture: { ...fixture, documents: [{
    file: { source: 'file', source_id: 'reference-source.md', title: 'Reference source', chunk_count: 1, last_modified: 1, space: 'Wenlan' }, content: sourceText,
  }] } });
  await page.goto('/');
  await openPrimaryDestination(page, 'Sources');
  await page.locator('.source-library').getByRole('button', { name: /Reference source/ }).click();
  // Imported sources now open as a full reading page from the library.
  const source = page.locator('.source-document-page');
  await expect(source.getByRole('heading', { name: 'Reference source', exact: true })).toBeVisible();
  await expect(source.locator('img')).toHaveCount(0);
  const link = source.getByRole('link', { name: 'Linked memory', exact: true });
  await link.focus();
  await expect(page.locator('[data-reference-preview]')).toContainText('Typed fixtures keep rendered');
  await page.keyboard.press('Tab');
  await expect(page.locator('[data-reference-preview] button').last()).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(source).toBeVisible();
  await expect(link).toBeFocused();
  await link.click();
  await expect(page.locator('.memory-detail-reading')).toContainText('Typed fixtures keep rendered');
  expect(controller.calls().filter(call => call.command === 'update_chunk')).toEqual([]);
  expect(external).toEqual([]);
});


test.describe('touch reference opening', () => {
  test.use({ hasTouch: true });
  test('the first touch shows memory preview before explicit opening', async ({ page }) => {
    const { editor } = await openReferences(page);
    await editor.getByRole('link', { name: '討論記憶', exact: true }).tap();
    const preview = page.getByRole('dialog', { name: '討論記憶', exact: true });
    await expect(preview).toContainText('Keep the original evidence');
    await expect(editor).toBeVisible();
    await preview.getByRole('button', { name: 'Open 討論記憶', exact: true }).tap();
    await expect(page.locator('.memory-detail-reading')).toContainText('Keep the original evidence');
  });
  test('touch in a source body previews before opening the linked memory', async ({ page }) => {
    const fixture = createSpacesNavigationFixture();
    await installTauriMock(page, { locale: 'en', rawActions: [], fixture: { ...fixture, documents: [{
      file: { source: 'file', source_id: 'touch-source.md', title: 'Touch source', chunk_count: 1, last_modified: 1, space: 'Wenlan' },
      content: 'Read [Linked memory](#memory:memory-1).',
    }] } });
    await page.goto('/');
    await openPrimaryDestination(page, 'Sources');
    await page.locator('.source-library').getByRole('button', { name: /Touch source/ }).click();
    const source = page.locator('.source-document-page');
    await expect(source.getByRole('heading', { name: 'Touch source', exact: true })).toBeVisible();
    await source.getByRole('link', { name: 'Linked memory', exact: true }).tap();
    const preview = page.locator('[data-reference-preview]');
    await expect(preview).toContainText('Typed fixtures keep rendered');
    await expect(source).toBeVisible();
    await preview.getByRole('button').last().tap();
    await expect(page.locator('.memory-detail-reading')).toContainText('Typed fixtures keep rendered');
  });

});
