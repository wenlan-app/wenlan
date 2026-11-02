// SPDX-License-Identifier: AGPL-3.0-only
import { openTopicMenu } from "./helpers/topicTools";
import { expect, test, type Page } from "@playwright/test";
import { openSpaceEntity } from "./helpers/spaceEntity";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

async function openSpaces(page: Page): Promise<void> {
  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Spaces", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
}

async function openWenlan(page: Page): Promise<void> {
  await openSpaces(page);
  await page.getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
}

test("closes the 899px drawer before history and moves focus safely at the 900px boundary", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 899, height: 900 });
  await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    localStorage: { "wenlan-sidebar-collapsed": "true", "wenlan-spaces-view-mode": "rows" },
  });
  await page.goto("/");
  await page.getByTitle("Show sidebar").click();
  await page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Spaces", exact: true }).click();
  await page.getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();

  await page.getByTitle("Show sidebar").click();
  await page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Spaces", exact: true }).focus();
  await page.keyboard.press("Escape");
  const primarySidebar = page.locator('aside[aria-label="Primary navigation"]');
  await expect(primarySidebar).toHaveAttribute("aria-hidden", "true");
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await expect(page.getByTitle("Show sidebar")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();

  await page.getByRole("button", { name: "Wenlan", exact: true }).click();
  await page.getByTitle("Show sidebar").click();
  const drawerDestination = page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Spaces", exact: true });
  await drawerDestination.focus();
  await page.setViewportSize({ width: 900, height: 900 });
  // A legacy collapsed preference migrates to a hidden labels sidebar.
  await expect(primarySidebar).toHaveAttribute("aria-hidden", "true");
  await expect(primarySidebar).toHaveCSS("width", "0px");
  await expect(drawerDestination).toBeHidden();
  await expect(page.getByTitle("Show sidebar")).toBeFocused();
  expect(JSON.parse(await page.evaluate(() => localStorage.getItem("wenlan-navigation-v1")!))).toMatchObject({ sidebar: { visible: false, mode: "labels" } });
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("an explicitly visible icon sidebar preserves destination focus at 900px", async ({ page }) => {
  await page.setViewportSize({ width: 899, height: 900 });
  await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    localStorage: { "wenlan-navigation-v1": JSON.stringify({ version: 2, visible: ["pages", "spaces", "sources"], sidebar: { visible: true, mode: "icons" } }) },
  });
  await page.goto("/");
  const sidebar = page.locator('aside[aria-label="Primary navigation"]');
  const toggle = page.getByTitle("Show sidebar");
  await toggle.click();
  const destination = page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Spaces", exact: true });
  await destination.focus();
  await expect(destination).toBeFocused();
  await page.setViewportSize({ width: 900, height: 900 });
  await expect(sidebar).toHaveAttribute("aria-hidden", "false");
  await expect(sidebar).toHaveCSS("width", "64px");
  await expect(destination).toBeVisible();
  await expect(destination).toBeFocused();
  await expect(page.getByTitle("Hide sidebar")).toBeVisible();
  expect(JSON.parse(await page.evaluate(() => localStorage.getItem("wenlan-navigation-v1")!))).toMatchObject({ sidebar: { visible: true, mode: "icons" } });
});

test("turns a non-Error Spaces rejection into a recoverable inline failure", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  await openSpaces(page);
  await page.evaluate(() => {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) throw new Error("Tauri internals are unavailable");
    const originalInvoke = internals.invoke;
    let rejectOnce = true;
    internals.invoke = (command, args, options) => {
      if (rejectOnce && command === "update_space") {
        rejectOnce = false;
        return Promise.reject("plain string rejection");
      }
      return originalInvoke(command, args, options);
    };
  });

  const archiveRow = page.getByTestId("space-row-space-archive");
  await archiveRow.hover();
  await archiveRow.getByRole("button", { name: "Actions for Archive" }).click();
  await page.getByRole("menuitem", { name: "Rename" }).click();
  await page.getByRole("textbox", { name: "Name", exact: true }).fill("Archive Retry");
  await page.getByRole("textbox", { name: "Name", exact: true }).press("Enter");
  await expect(page.getByRole("alert")).toContainText("could not be saved");
  await expect(page.getByRole("alert")).not.toContainText("plain string rejection");
  await expect(page.getByRole("textbox", { name: "Name", exact: true })).toHaveValue("Archive Retry");
  await page.getByRole("textbox", { name: "Name", exact: true }).press("Enter");
  await expect(page.getByRole("button", { name: "Archive Retry", exact: true })).toBeVisible();
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("retries Topic load, retains a failed observation draft, and retries two-step deletion", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  await openWenlan(page);

  controller.failNext("get_entity_detail_cmd", "entity offline");
  await openSpaceEntity(page, "Ada Lovelace");
  await expect(page.getByText("Couldn't load this topic.")).toBeVisible();
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();

  await page.getByRole("button", { name: "Edit information — Wrote the first published algorithm", exact: true }).click();
  const observation = page.getByRole("textbox", { name: "Edit information" });
  controller.failNext("update_observation_cmd", "observation write failed");
  await observation.fill("Observation draft survives");
  await expect(page.getByRole("alert")).toContainText("Couldn't save. Try again.");
  await expect(observation).toHaveValue("Observation draft survives");
  await page.getByRole("button", { name: "Retry save", exact: true }).click();
  await expect(observation).toHaveValue("Observation draft survives");
  await observation.press("Escape");
  await expect(observation).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Edit information — Observation draft survives", exact: true })).toBeVisible();
  expect(controller.calls().filter(({ command }) => command === "update_observation_cmd")).toHaveLength(2);

  controller.failNext("delete_entity_cmd", "delete failed");
  const topicMenu = await openTopicMenu(page);
  await topicMenu.getByRole("menuitem", { name: "Delete topic", exact: true }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Couldn't save. Try again.");
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Topics" })).toBeVisible();

  expect(controller.calls().filter(({ command }) => command === "get_entity_detail_cmd").length).toBeGreaterThanOrEqual(2);
  expect(controller.calls().filter(({ command }) => command === "update_observation_cmd")).toHaveLength(2);
  expect(controller.calls().filter(({ command }) => command === "delete_entity_cmd")).toHaveLength(2);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
