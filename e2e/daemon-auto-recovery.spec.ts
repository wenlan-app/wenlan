// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";

import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

test("keeps the compact service row quiet during automatic recovery and explains a failure", async ({ page }, info) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    localStorage: {
      "wenlan-navigation-v1": JSON.stringify({
        version: 1,
        visible: ["pages", "spaces", "graph", "sources"],
        sidebar: { visible: true, mode: "icons" },
      }),
    },
  });
  await page.addInitScript(() => {
    const browserWindow = window as typeof window & {
      __daemonRecoveryCalls: { statusReads: number; restarts: number };
      __finishDaemonRecovery?: () => void;
    };
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) throw new Error("Tauri mock was not installed before bootstrap");
    const originalInvoke = internals.invoke.bind(internals);
    let failedRestarts = 1;
    browserWindow.__daemonRecoveryCalls = { statusReads: 0, restarts: 0 };
    internals.invoke = async (command: string, args?: unknown) => {
      if (command === "daemon_version_status") {
        browserWindow.__daemonRecoveryCalls.statusReads += 1;
        return { daemon: "0.19.3", app: "0.20.0", matched: false, owner: "launchd", program: null };
      }
      if (command === "restart_daemon") {
        browserWindow.__daemonRecoveryCalls.restarts += 1;
        if (failedRestarts > 0) {
          await new Promise<void>((resolve) => { browserWindow.__finishDaemonRecovery = resolve; });
          failedRestarts -= 1;
          throw "daemon-restart:sidecar-still-running: pid 12345 is still alive";
        }
        return "0.20.0";
      }
      return originalInvoke(command, args);
    };
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");

  // Wait until the watcher has started its safe retry. The compact rail has no
  // warning while recovery is in progress; only the service icon becomes amber
  // after the mocked restart reports the mismatch again.
  await expect.poll(() => page.evaluate(() => {
    const browserWindow = window as typeof window & {
      __daemonRecoveryCalls: { statusReads: number };
    };
    return browserWindow.__daemonRecoveryCalls.statusReads;
  }))
    .toBeGreaterThan(0);
  await expect.poll(() => page.evaluate(() => {
    const browserWindow = window as typeof window & {
      __daemonRecoveryCalls: { restarts: number };
    };
    return browserWindow.__daemonRecoveryCalls.restarts;
  }))
    .toBeGreaterThan(0);
  await expect(page.getByTestId("service-status")).toHaveCount(0);

  await page.evaluate(() => {
    const browserWindow = window as typeof window & {
      __finishDaemonRecovery?: () => void;
    };
    if (!browserWindow.__finishDaemonRecovery) throw new Error("automatic recovery is not pending");
    browserWindow.__finishDaemonRecovery();
  });
  await openPrimaryDestination(page, "Wiki");
  const status = page.getByTestId("service-status");
  await expect(status).toHaveAttribute("data-kind", "outOfDate");
  if (await status.getAttribute("aria-expanded") === "false") await status.click();
  await expect(page.getByTestId("service-status-popover")).toContainText("0.19.3");
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("Wenlan couldn't confirm the background service stopped");
  await expect(alert).not.toContainText("12345");
  await expect(alert).not.toContainText("still alive");
  await expect(page.getByTestId("service-status-action")).toBeEnabled();
  await page.screenshot({ path: info.outputPath("compact-service-recovery-failed.png"), fullPage: false });

  await page.getByTestId("service-status-action").click();
  await expect(page.getByTestId("service-status")).toHaveCount(0);
  expect(await page.evaluate(() => {
    const browserWindow = window as typeof window & {
      __daemonRecoveryCalls: { restarts: number };
    };
    return browserWindow.__daemonRecoveryCalls.restarts;
  })).toBe(2);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
