// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from '@playwright/test';
import { installTauriMock, collectBrowserErrors } from './tauriMock';

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
