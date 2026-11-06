// SPDX-License-Identifier: AGPL-3.0-only
import { devices, expect, test } from '@playwright/test';
import { collectBrowserErrors, installTauriMock } from './tauriMock';

for (const [platform, userAgent] of [
  ['Windows', devices['Desktop Chrome'].userAgent],
  ['macOS', devices['Desktop Safari'].userAgent],
  ['Linux', 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36'],
] as const) {
  test(`first launch stays usable before daemon readiness on ${platform}`, async ({ page }, info) => {
    await page.setViewportSize({ width: 1173, height: 672 });
    await page.route('**/favicon.ico', route => route.fulfill({ status: 204 }));
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale: 'zh-Hant', rawActions: [] });
    await page.addInitScript(({ userAgent }) => {
      Object.defineProperty(navigator, 'userAgent', { get: () => userAgent });
      const invoke = window.__wenlanTauriInvoke;
      let ready!: (show: boolean) => void;
      const waiting = new Promise<boolean>(resolve => { ready = resolve; });
      Object.assign(window, { releaseFirstRunDaemon: () => ready(true) });
      window.__wenlanTauriInvoke = (command, args) => {
        if (command === 'should_show_wizard') return waiting;
        return invoke(command, args);
      };
    }, { userAgent });
    await page.goto('/');
    await expect(page.getByTestId('notes-welcome')).toBeVisible();
    await expect(page.getByTestId('notes-welcome-empty')).toBeEnabled();
    await expect(page.getByText(/Starting Wenlan|正在啟動文瀾/)).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('first-run.png') });
    await page.getByTestId('notes-welcome-empty').click();
    await expect(page.getByTestId('notes-welcome')).toHaveCount(0);
    await expect(page.getByRole('navigation')).not.toHaveCount(0);
    expect(controller.calls().filter(call => call.command === 'set_setup_completed')).toHaveLength(0);
    await page.screenshot({ path: info.outputPath('workspace-before-ready.png') });
    await page.evaluate(() => (window as unknown as { releaseFirstRunDaemon: () => void }).releaseFirstRunDaemon());
    await expect.poll(() => controller.calls().filter(call => call.command === 'set_setup_completed').length).toBe(1);
    await expect(page.getByTestId('notes-welcome')).toHaveCount(0);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
