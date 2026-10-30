// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from '@playwright/test';
import { installTauriMock, collectBrowserErrors } from './tauriMock';
import { openWikiNote } from './helpers/wikiWorkspace';
import { createSpacesNavigationFixture } from './fixtures/spacesNavigation';

// Fixture-only rendering: this does not exercise native APIs or live OAuth.
const cases = ['en'] as const;
for (const locale of cases) for (const width of [1440, 375]) {
  test(`${locale} ${width}: the example wiki opens the app and Start my own leaves it`, async ({ page, baseURL }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    const errors = collectBrowserErrors(page);
    const external: string[] = [];
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.origin === new URL(baseURL!).origin) return route.continue();
      external.push(url.origin);
      return route.abort();
    });
    await installTauriMock(page, { locale, rawActions: [] });
    let completed = false;
    await page.exposeBinding('__sampleFirstRunGate', (_source, command: string) => {
      if (command === 'should_show_wizard') return !completed;
      completed = true;
      return null;
    });
    await page.addInitScript(() => {
      const invoke = window.__wenlanTauriInvoke;
      window.__wenlanTauriInvoke = (command, args) =>
        ['should_show_wizard', 'set_setup_completed'].includes(command)
          ? (window as unknown as { __sampleFirstRunGate: (command: string) => Promise<unknown> }).__sampleFirstRunGate(command)
          : invoke(command, args);
    });
    await page.goto('/');
    await expect(page.getByTestId('notes-welcome')).toBeVisible();
    await page.getByTestId('notes-welcome-example').click();
    // The example opens the app itself, as a separate wiki with its own line at the foot.
    await expect(page.getByTestId('example-wiki-banner')).toBeVisible();
    expect(completed).toBe(true);
    await expect(page.locator('main').getByText('v2 launch plan', { exact: true }).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('example-wiki.png'), fullPage: true, animations: 'disabled' });
    await page.getByTestId('example-wiki-start').click();
    await expect(page.getByTestId('example-wiki-banner')).toHaveCount(0);
    await expect(page.getByRole('dialog')).toBeVisible();
    await expect(page.locator('[data-testid="setup-wizard"]')).toHaveCount(0);
    expect(await page.locator('vite-error-overlay').count()).toBe(0);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
    expect(external).toEqual([]);
  });
}

test('Start empty opens the shared editor and the first note survives reopening the app', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const errors = collectBrowserErrors(page);
  const populated = createSpacesNavigationFixture();
  const fixture = {
    ...populated, pages: [], memories: [], entities: [], entityDetails: [], refinements: [],
    distillReview: { ...populated.distillReview, pending: [], stale_pages: [], orphan_topics: [] },
  };
  const runtime = await installTauriMock(page, { fixture, locale: 'en', rawActions: [] });
  let completed = false;
  await page.exposeBinding('__emptyFirstRunGate', (_source, command: string) => {
    if (command === 'should_show_wizard') return !completed;
    completed = true;
    return null;
  });
  await page.addInitScript(() => {
    const invoke = window.__wenlanTauriInvoke;
    window.__wenlanTauriInvoke = (command, args) =>
      ['should_show_wizard', 'set_setup_completed'].includes(command)
        ? (window as unknown as { __emptyFirstRunGate: (command: string) => Promise<unknown> }).__emptyFirstRunGate(command)
        : invoke(command, args);
  });
  await page.goto('/');
  await page.getByTestId('notes-welcome-empty').click();
  await expect(page.getByTestId('notes-welcome')).toHaveCount(0);
  const group = page.locator('[data-note-group-id="primary"]');
  await group.getByRole('textbox', { name: 'Title', exact: true }).fill('My first note');
  const editor = group.getByRole('textbox', { name: 'Content', exact: true });
  await expect(editor).toHaveClass(/cm-content/);
  await editor.fill('你好\nA first note.');
  await group.locator('.note-tab[data-active="true"] .note-tab-close').click();
  await expect.poll(() => runtime.calls().filter(call => call.command === 'publish_page_draft').length).toBe(1);
  await page.reload();
  await openWikiNote(page, 'My first note');
  await expect(group.locator('.page-detail')).toContainText('你好');
  await expect(group.locator('.page-detail')).toContainText('A first note.');
  expect(completed).toBe(true);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
