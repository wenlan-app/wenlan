// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator, type Page } from '@playwright/test';
import { collectBrowserErrors, installTauriMock } from './tauriMock';
import { openPrimaryDestination } from './helpers/primaryNavigation';

async function centerClick(page: Page, target: Locator) {
  await target.scrollIntoViewIfNeeded();
  const box = await target.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
}

for (const width of [1280, 375]) {
  test(`Space cards open from description, count, padding, icon and keyboard at ${width}`, async ({ page }, info) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: 'en', rawActions: [], localStorage: { 'wenlan-spaces-view-mode': 'cards', 'wenlan-theme': width === 375 ? 'dark' : 'light' } });
    await page.goto('/');
    await openPrimaryDestination(page, 'Spaces');
    const card = page.getByTestId('space-card-space-wenlan');
    await expect(card).toBeVisible();
    await page.screenshot({ path: info.outputPath('cards.png') });
    const regions = ['description', 'count', 'padding', 'icon', 'title', 'Enter', 'Space'] as const;
    for (const region of regions) {
      await test.step(region, async () => {
        await expect(card).toBeVisible();
        if (region === 'description') await centerClick(page, card.locator('.asset-card-context'));
        else if (region === 'count') await centerClick(page, card.getByTestId('space-card-pages'));
        else if (region === 'icon') await centerClick(page, card.locator('.space-card-mark'));
        else if (region === 'padding') {
          await card.scrollIntoViewIfNeeded();
          const box = (await card.boundingBox())!;
          await page.mouse.click(box.x + box.width - 8, box.y + box.height - 8);
        } else {
          const open = card.getByRole('button', { name: 'Open Wenlan', exact: true });
          if (region === 'title') await open.click();
          else {
            await open.focus();
            await expect(open).toBeFocused();
            await open.press(region === 'Space' ? ' ' : 'Enter');
          }
        }
        await expect(page.getByRole('heading', { name: 'Wenlan', level: 1, exact: true })).toBeVisible();
        await page.getByRole('group', { name: 'History navigation', exact: true }).getByRole('button', { name: 'Back', exact: true }).click();
      });
    }
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });

  test(`Space card menu and confirmation remain independent at ${width}`, async ({ page }, info) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width, height: 900 });
    const controller = await installTauriMock(page, { locale: 'en', rawActions: [], localStorage: { 'wenlan-spaces-view-mode': 'cards' } });
    await page.goto('/');
    await openPrimaryDestination(page, 'Spaces');
    const card = page.getByTestId('space-card-space-research');
    const menu = card.getByRole('button', { name: 'Actions for Research' });
    await card.hover();
    await menu.click();
    await page.screenshot({ path: info.outputPath('menu.png') });
    await page.getByRole('menuitem', { name: /^(Unstar|Star)$/ }).click();
    await expect(card).toBeVisible();
    await card.hover();
    await menu.click();
    await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
    await expect(card.getByRole('textbox', { name: 'Name', exact: true })).toBeVisible();
    await card.getByRole('button', { name: 'Cancel', exact: true }).click();
    await card.hover();
    await menu.click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await page.screenshot({ path: info.outputPath('confirmation.png') });
    await page.getByRole('alertdialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(card).toBeVisible();
    expect(controller.calls().filter(c => c.command === 'delete_space')).toHaveLength(0);
    await card.hover();
    await menu.click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(card).toHaveCount(0);
    await expect(page.getByTestId('spaces-cards')).toBeVisible();
    expect(controller.calls().filter(c => c.command === 'delete_space')).toHaveLength(1);
    expect(controller.calls().filter(c => c.command === 'update_space')).toHaveLength(0);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}

test('Space cards reorder by drag without opening the Space', async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 900 });
  const controller = await installTauriMock(page, { locale: 'en', rawActions: [], localStorage: { 'wenlan-spaces-view-mode': 'cards' } });
  await page.goto('/');
  await openPrimaryDestination(page, 'Spaces');
  const source = page.getByTestId('space-card-space-personal');
  const target = page.getByTestId('space-card-space-archive');
  const from = (await source.boundingBox())!;
  const to = (await target.boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 20, from.y + from.height / 2, { steps: 4 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 8 });
  await page.mouse.up();
  await expect.poll(() => controller.calls().filter(c => c.command === 'reorder_space').length).toBe(1);
  await expect(page.getByTestId('spaces-cards')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Personal', level: 1, exact: true })).toHaveCount(0);
  const names = await page.getByTestId('spaces-cards').locator('.asset-card-title').allTextContents();
  expect(names.indexOf('Archive')).toBeLessThan(names.indexOf('Personal'));
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
