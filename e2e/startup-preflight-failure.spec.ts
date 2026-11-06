// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

test("first run explains a terminal startup preflight failure and allows leaving setup", async ({ page }, info) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.route("**/favicon.ico", route => route.fulfill({ status: 204 }));
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.exposeBinding("__startupPreflightState", () => "failed");
  await page.addInitScript(() => {
    const invoke = window.__wenlanTauriInvoke;
    window.__wenlanTauriInvoke = (command, args) => {
      if (command === "get_startup_preflight_status") {
        return (window as unknown as { __startupPreflightState: () => Promise<string> })
          .__startupPreflightState();
      }
      if (command === "should_show_wizard") return new Promise<boolean>(() => {});
      return invoke(command, args);
    };
  });

  await page.goto("/");

  await expect(page.getByTestId("startup-preflight-failure")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Wenlan couldn't repair its startup configuration" })).toBeVisible();
  await expect(page.getByText("Quit Wenlan, then reopen it to try the repair again.")).toBeVisible();
  const open = page.getByRole("button", { name: "Open Wenlan", exact: true });
  await expect(open).toBeVisible();
  // The preflight cannot repair itself during this launch. Opening the
  // workspace is still safe; it must not attempt repair or save completion.
  await expect(page.getByRole("button")).toHaveCount(1);
  await page.screenshot({ path: info.outputPath("startup-preflight-failure.png"), animations: "disabled" });
  await open.click();
  await expect(page.getByTestId("startup-preflight-failure")).toHaveCount(0);
  await expect(page.getByTestId("wizard-action-bar")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Pages can’t load yet" })).toBeVisible();
  expect(controller.calls().filter(call => call.command === "set_setup_completed" || call.command === "start_daemon_sidecar")).toEqual([]);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
