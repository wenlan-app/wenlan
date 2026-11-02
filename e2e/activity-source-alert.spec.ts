// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { resources } from "../src/i18n/resources";
import type { RegisteredSource } from "../src/lib/tauri";

// A source Wenlan cannot reach needs the person, so Activity says it in its
// own place in the sidebar, never on Home, and opens that folder.
for (const cfg of [
  { locale: "en", theme: "light", width: 1440 },
  { locale: "zh-Hans", theme: "dark", width: 1280 },
  { locale: "zh-Hant", theme: "light", width: 375 },
] as const) {
  test(`Activity names a source that cannot be reached and opens it ${cfg.locale} ${cfg.width}`, async ({ page }, info) => {
    const t = resources[cfg.locale].translation;
    const errors = collectBrowserErrors(page);
    await installTauriMock(page, { fixture: createSpacesNavigationFixture(), locale: cfg.locale, rawActions: [], localStorage: {
      "wenlan-theme": cfg.theme,
      "onboarding:gettingStartedDismissed": "1",
    } });
    await page.setViewportSize({ width: cfg.width, height: 900 });
    const sources: RegisteredSource[] = [
      { id: "drive", source_type: "directory", path: "/Volumes/Backup/Research", status: { Unavailable: "path not found" }, last_sync: 1791680000, file_count: 4, memory_count: 2 },
      { id: "notes", source_type: "obsidian", path: "/fixture/Reading Vault", status: "Active", last_sync: 1791680000, file_count: 2, memory_count: 3 },
    ];
    await page.exposeBinding("__alertInvoke", (_source, command: string) => {
      if (command === "list_registered_sources") return sources;
      if (command === "list_indexed_files") return [];
      if (command === "read_source_dir") throw new Error("path not found");
      throw new Error(`Unexpected command: ${command}`);
    });
    await page.addInitScript(() => {
      const bridge = (window as any).__TAURI_INTERNALS__, original = bridge.invoke;
      bridge.invoke = (command: string, args: unknown) => ["list_registered_sources", "list_indexed_files", "read_source_dir"].includes(command)
        ? (window as any).__alertInvoke(command, args) : original(command, args);
    });
    await page.goto("/");

    if (cfg.width < 900) await page.getByRole("button", { name: t.sidebar.show, exact: true }).click();
    const label = t.activityStatus.sourcesBroken_one.replace("{{count}}", "1");
    const trigger = page.getByTestId("activity-status");
    await expect(trigger).toHaveAccessibleName(label);
    await expect(trigger).toHaveAttribute("data-state", "source");
    await expect(page.locator(".home-page, main").getByText(label)).toHaveCount(0);
    await trigger.click();
    const summary = page.getByTestId("activity-summary-sources");
    await expect(summary).toContainText("Research");
    await expect(summary).toContainText(t.activityStatus.sourceUnavailable);
    await expect(summary).not.toContainText("Reading Vault");
    await page.evaluate(() => document.fonts.ready);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath("activity-source-alert.png") });

    await summary.getByRole("button", { name: t.activityStatus.sourceOpen, exact: true }).click();
    await expect(page.getByTestId("activity-summary")).toHaveCount(0);
    await expect(page.locator(".source-collection-breadcrumbs [aria-current='page']")).toHaveText("Research");
    await page.screenshot({ path: info.outputPath("activity-source-opened.png") });
    expect(errors.pageErrors).toEqual([]);
  });
}
