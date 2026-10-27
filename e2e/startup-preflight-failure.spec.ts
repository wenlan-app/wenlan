// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

test("first run immediately explains a terminal startup preflight failure", async ({ page }, info) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [] });
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
  await expect(page.getByRole("button")).toHaveCount(0);
  await page.screenshot({ path: info.outputPath("startup-preflight-failure.png"), animations: "disabled" });
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
