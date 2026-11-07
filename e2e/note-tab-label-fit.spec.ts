// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from '@playwright/test';
import { installTauriMock } from './tauriMock';
import { createReviewDecisionFixture } from './fixtures/reviewDecisions';
import { openWikiNote } from './helpers/wikiWorkspace';

test('a long active title keeps its beginning and close control visible beside the shared inspector', async ({ page }, info) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const base = createReviewDecisionFixture('wiki-folders');
  const title = 'Does retrieving more passages always improve answer quality?';
  const fixture = { ...base, pages: base.pages.map((item, index) => index === 0 ? { ...item, title } : item) };
  await installTauriMock(page, { fixture, locale: 'en', rawActions: [] });
  await page.goto('/');
  await openWikiNote(page, title);
  await openWikiNote(page, 'Why fixtures stay deterministic');
  await page.getByRole('tab', { name: 'Why fixtures stay deterministic', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Move to right group', exact: true }).click();
  const primary = page.locator('[data-note-group-id="primary"]');
  await primary.getByRole('tab', { name: title, exact: true }).click();
  await page.getByRole('button', { name: 'Info', exact: true }).click();
  await expect(page.locator('.page-info-drawer')).toBeVisible();
  await expect.poll(() => primary.locator('.note-tabs-list').evaluate(strip => {
    const tab = strip.querySelector<HTMLElement>('.note-tab[data-active="true"]')!;
    const bounds = strip.getBoundingClientRect(), active = tab.getBoundingClientRect();
    const close = tab.querySelector<HTMLElement>('.note-tab-close')!.getBoundingClientRect();
    return active.left >= bounds.left - 1 && active.right <= bounds.right + 1 && close.right <= bounds.right + 1;
  })).toBe(true);
  await page.screenshot({ path: info.outputPath('shared-inspector-readable-tabs.png') });
});
