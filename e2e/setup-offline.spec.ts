// SPDX-License-Identifier: AGPL-3.0-only
import { devices, expect, test } from '@playwright/test';
import { collectBrowserErrors, installTauriMock } from './tauriMock';
import { BOOT_QUERY_RETRY, bootQueryRetryDelay } from '../src/lib/bootRetryPolicy';

test.use({ userAgent: devices['Desktop Chrome'].userAgent });

for (const viewport of [{ width: 1173, height: 672 }, { width: 640, height: 420 }, { width: 390, height: 420 }]) {
  test(`failed setup remains escapable at ${viewport.width}x${viewport.height}`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    await page.route('**/favicon.ico', route => route.fulfill({ status: 204 }));
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, {
      locale: 'zh-Hant', rawActions: [],
      failures: [
        { command: 'should_show_wizard', message: 'connection refused', times: 100 },
        { command: 'wire_state', message: 'wenlan-server stopped: exit code -1073741515 (0xC0000135). A required Windows runtime DLL is missing.', times: 100 },
        { command: 'start_daemon_sidecar', message: 'Service start failed (0xC0000135)', times: 1 },
      ],
    });
    await page.clock.install();
    await page.goto('/');
    // Drive the real retry policy in virtual time, without replacing the gate.
    for (let attempt = 1; attempt <= BOOT_QUERY_RETRY + 1; attempt++) {
      await expect.poll(() => controller.calls().filter(call => call.command === 'should_show_wizard').length).toBeGreaterThanOrEqual(attempt);
      await page.clock.runFor(bootQueryRetryDelay(attempt - 1) + 100);
    }
    await page.getByRole('button', { name: '檢查連線' }).click();
    const open = page.getByRole('button', { name: '先開啟文瀾', exact: true });
    await expect(open).toBeVisible();
    await expect(page.getByRole('heading', { name: '暫時無法完成設定' })).toBeVisible();
    await expect(page.getByText(/0xC0000135/)).not.toBeVisible();
    await expect(page.getByText('正在設定', { exact: true })).toHaveCount(0);
    const box = await open.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.y).toBeGreaterThanOrEqual(0);
    expect(box!.y + box!.height).toBeLessThanOrEqual(viewport.height);
    expect(box!.x + box!.width).toBeLessThanOrEqual(viewport.width);
    await page.screenshot({ path: testInfo.outputPath('setup-offline.png') });
    await page.getByText('查看錯誤資訊', { exact: true }).click();
    await expect(page.getByText(/0xC0000135/)).toBeVisible();
    await expect(open).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('setup-offline-details.png') });
    await page.getByRole('button', { name: '重試連線', exact: true }).click();
    await expect.poll(() => controller.calls().filter(call => call.command === 'start_daemon_sidecar').length).toBe(1);
    await expect(page.getByRole('button', { name: '重試連線', exact: true })).toBeEnabled();
    await expect(open).toBeVisible();
    await open.click();
    await expect(page.getByTestId('setting-up-tasks')).toHaveCount(0);
    await expect(page.getByTestId('wizard-action-bar')).toHaveCount(0);
    await expect(page.getByRole('heading', { name: '筆記暫時無法載入' })).toBeVisible();
    expect(controller.calls().filter(call => call.command === 'set_setup_completed')).toEqual([]);
    expect(errors.pageErrors).toEqual([]);
    // The launch pin-fill read also meets the intentionally unavailable
    // daemon. It logs this refusal and does not attempt any pin writes.
    expect(errors.consoleErrors.filter(message => !message.startsWith(
      'routing: could not read setup status before the launch pin fill ConfiguredTauriFailureError: should_show_wizard: connection refused',
    ))).toEqual([]);
    await page.screenshot({ path: testInfo.outputPath('workspace-offline.png') });
  });
}
