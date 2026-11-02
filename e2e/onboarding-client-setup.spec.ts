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
    // The example opens on its pages; a narrow window shows them behind the folders button.
    if (width === 375) await page.locator('.wiki-workspace-folders-toggle').click();
    await expect(page.locator('main').getByText('v2 launch plan', { exact: true }).filter({ visible: true }).first()).toBeVisible();
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

const emptyLibrary = () => {
  const populated = createSpacesNavigationFixture();
  return {
    ...populated, pages: [], memories: [], entities: [], entityDetails: [], refinements: [],
    distillReview: { ...populated.distillReview, pending: [], stale_pages: [], orphan_topics: [] },
  };
};

test('Start empty opens the draft editor and the first page survives reopening the app', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { fixture: emptyLibrary(), locale: 'en', rawActions: [] });
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
  await group.getByRole('textbox', { name: 'Title', exact: true }).fill('My first page');
  await group.getByRole('textbox', { name: 'Content', exact: true }).fill('你好\nA first page.');
  await group.locator('.note-tab[data-active="true"] .note-tab-close').click();
  await expect.poll(() => runtime.calls().filter(call => call.command === 'publish_page_draft').length).toBe(1);
  await page.reload();
  await openWikiNote(page, 'My first page');
  await expect(group.locator('.page-detail')).toContainText('你好');
  await expect(group.locator('.page-detail')).toContainText('A first page.');
  expect(completed).toBe(true);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test('See example saves the right group\'s unsaved draft first, creates nothing inside it, and leaving starts over', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { fixture: emptyLibrary(), locale: 'en', rawActions: [], gettingStarted: true, delays: { update_page_draft: 300 } });
  const left = page.locator('[data-note-group-id="primary"]');
  const right = page.locator('[data-note-group-id="secondary"]');
  const savedText = () => runtime.calls()
    .filter(call => ['create_page_draft', 'update_page_draft', 'publish_page_draft'].includes(call.command))
    .map(call => JSON.stringify(call.args));
  await page.goto('/');
  // The checklist offers the example only while the home is showing, so the
  // central group has nothing unsaved; the right group can still be mid-edit.
  // A draft moved there while still blank stays a draft (a titled one is
  // published, which would finish the checklist's pages step).
  await page.getByRole('button', { name: 'New page', exact: true }).first().click();
  await left.getByRole('tab', { selected: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Move to right group', exact: true }).click();
  await expect(left.getByRole('tab')).toHaveCount(0);
  await right.getByRole('textbox', { name: 'Content', exact: true }).fill('Right group text, typed last.');
  await page.getByTestId('getting-started-pages-sample').click();
  await expect(page.getByTestId('example-wiki-banner')).toBeVisible();
  // The draft reached the user's own library before the example took over.
  expect(savedText().some(args => args.includes('Right group text, typed last.'))).toBe(true);
  // Nothing of the user's wiki is open in the example, and nothing can be created there.
  await expect(right).toHaveCount(0);
  await expect(left.getByRole('tab')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New page', exact: true })).toHaveCount(0);
  await expect(page.locator('main').getByText('v2 launch plan', { exact: true }).filter({ visible: true }).first()).toBeVisible();
  const draftsInExample = runtime.calls().filter(call => call.command === 'create_page_draft').length;
  await page.getByTestId('example-wiki-back').click();
  await expect(page.getByTestId('example-wiki-banner')).toHaveCount(0);
  // Back in the user's wiki the workspace starts over: no tab from either wiki.
  await expect(left.getByRole('tab')).toHaveCount(0);
  await expect(right).toHaveCount(0);
  expect(runtime.calls().filter(call => call.command === 'create_page_draft').length).toBe(draftsInExample);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
