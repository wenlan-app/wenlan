// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from '@playwright/test';
import { installTauriMock, collectBrowserErrors } from './tauriMock';
import { enFirstUse } from '../src/components/onboarding/firstUseCopy';

const enSetupStart = 'Get started';

// Fixture-only rendering: this does not exercise native APIs or live OAuth.
const cases = [['en', enFirstUse]] as const;
for (const [locale, copy] of cases) for (const width of [1440, 375]) {
  test(`${locale} ${width}: sample ends by opening pages`, async ({ page, baseURL }, info) => {
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
    await page.getByRole('button', { name: copy.guide.tryExample, exact: true }).click();
    await expect(page.getByTestId('first-use-sample')).toBeVisible();
    expect(completed).toBe(false);
    await page.getByRole('button', { name: copy.sample.skipToResult, exact: true }).click();
    await page.getByRole('button', { name: copy.sample.useWithAi, exact: true }).click();
    await page.getByRole('tab', { name: 'ChatGPT', exact: true }).click();
    await expect(page.getByText(copy.sample.chatgptNote, { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: `${copy.sample.copyCommand}: ${copy.sample.handoffLabel}`, exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: `${copy.sample.copyCommand}: ${copy.sample.briefLabel}`, exact: true })).toBeVisible();
    await page.locator('.fus-use').scrollIntoViewIfNeeded();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('sample-chatgpt.png'), fullPage: true, animations: 'disabled' });
    await page.getByRole('tab', { name: 'Codex', exact: true }).click();
    await expect(page.getByRole('button', { name: `${copy.sample.copyCommand}: ${copy.sample.handoffLabel}`, exact: true })).toBeVisible();
    await page.getByRole('tab', { name: 'ChatGPT', exact: true }).click();
    await expect(page.getByRole('button', { name: /^Connect my AI$/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Bring my data$/ })).toHaveCount(0);
    await page.getByRole('button', { name: enSetupStart, exact: true }).click();
    await expect(page.getByTestId('first-use-sample')).toHaveCount(0);
    expect(completed).toBe(true);
    await expect(page.locator('[data-testid="setup-wizard"]')).toHaveCount(0);
    expect(await page.locator('vite-error-overlay').count()).toBe(0);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
    expect(external).toEqual([]);
  });
}
