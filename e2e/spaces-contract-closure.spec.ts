// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

const RECENT_SPACES_KEY = "wenlan:recent-spaces:v1";

async function openSpaces(page: Page): Promise<void> {
  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Spaces", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
}

async function openSpace(page: Page, name = "Wenlan"): Promise<void> {
  await openSpaces(page);
  await page
    .getByTestId("space-row-space-wenlan")
    .getByRole("button", { name, exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name })).toBeVisible();
}

async function expectNoIndexLabel(page: Page): Promise<void> {
  const accessibilitySnapshot = await page.locator("body").ariaSnapshot();
  expect(accessibilitySnapshot).not.toMatch(/\bIndex\b|索引/iu);
}

test("opens Spaces by stable id, prunes missing history, and preserves selection through rename", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const recentHistory = JSON.stringify({
    version: 1,
    entries: [
      { id: "space-wenlan", name: "Old Wenlan name", visitedAt: Date.now() - 1_000 },
      { id: "space-missing", name: "Missing", visitedAt: Date.now() - 2_000 },
    ],
  });
  await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    localStorage: { [RECENT_SPACES_KEY]: recentHistory, "wenlan-spaces-view-mode": "rows" },
  });
  await page.goto("/");

  await expectNoIndexLabel(page);
  await openSpaces(page);
  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  await page.getByTestId("space-row-space-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Spaces", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(navigation.getByRole("button", { name: "Spaces", exact: true })).not.toHaveAttribute("aria-pressed");
  await expect.poll(() => page.evaluate((key) => localStorage.getItem(key), RECENT_SPACES_KEY)).not.toContain("space-missing");
  await expectNoIndexLabel(page);

  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { level: 1, name: "Spaces", exact: true })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
  await openSpaces(page);
  const wenlanRow = page.getByTestId("space-row-space-wenlan");
  await wenlanRow.hover();
  await wenlanRow.getByRole("button", { name: "Actions for Wenlan" }).click();
  await page.getByRole("menuitem", { name: "Rename" }).click();
  const rename = wenlanRow.getByRole("textbox", { name: "Name", exact: true });
  await rename.fill("Wenlan Core");
  await rename.press("Enter");
  await expect(wenlanRow.getByRole("button", { name: "Wenlan Core", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate((key) => localStorage.getItem(key), RECENT_SPACES_KEY)).toContain('"id":"space-wenlan","name":"Wenlan Core"');

  await wenlanRow.getByRole("button", { name: "Wenlan Core", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan Core" })).toBeVisible();
  const selectedDestination = navigation.getByRole("button", { name: "Spaces", exact: true });
  await expect(selectedDestination).toHaveAttribute("aria-current", "page");
  await expect(selectedDestination).not.toHaveAttribute("aria-pressed");
  await expectNoIndexLabel(page);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("keeps inline Space edits after failure and keeps standalone memories absent and preserves delete cancellation", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  await openSpace(page);

  const titleEditor = page.getByRole("textbox", { name: "Name", exact: true });
  const descriptionEditor = page.getByRole("textbox", { name: "Edit description" });
  await expect(titleEditor).toHaveValue("Wenlan");
  await expect(descriptionEditor).toHaveValue("Editorial memory system");
  await descriptionEditor.fill("Failed description draft");
  const updatesBefore = controller.calls().filter(({ command }) => command === "update_space").length;
  controller.failNext("update_space", "identity write failed");
  await descriptionEditor.press("Tab");
  await expect(page.getByRole("alert")).toContainText("Could not save this change");
  await expect(descriptionEditor).toHaveValue("Failed description draft");
  const failedUpdates = controller.calls().filter(({ command }) => command === "update_space");
  expect(failedUpdates).toHaveLength(updatesBefore + 1);
  expect(failedUpdates.at(-1)?.args).toEqual({
    description: "Failed description draft",
    name: "Wenlan",
    newName: "Wenlan",
  });
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(descriptionEditor).toHaveValue("Failed description draft");
  await titleEditor.fill("Keyboard title");
  await titleEditor.press("Enter");
  await expect(page.getByRole("heading", { level: 1, name: "Keyboard title" })).toBeVisible();

  await page.reload();
  await openSpace(page, "Keyboard title");

  await expect(page.getByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "Sources" })).toBeVisible();
  await expect(page.locator(".space-dossier-archive, .space-dossier-entities, .space-dossier-review")).toHaveCount(0);

  const deleteCallsBefore = controller.calls().filter(({ command }) => command === "delete_space").length;
  await page.getByRole("button", { name: "Actions for Keyboard title" }).click();
  await page.getByRole("menuitem", { name: "Delete space" }).click();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("button", { name: "Confirm delete space" })).toHaveCount(0);
  expect(controller.calls().filter(({ command }) => command === "delete_space")).toHaveLength(deleteCallsBefore);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("keeps and discards suggested Spaces through their dossier controls", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  await openSpaces(page);

  // Suggestions rest collapsed in the header disclosure: confirmed Spaces stay
  // visible while suggestion rows are hidden until the disclosure opens.
  const suggestionsDisclosure = page.locator("summary").filter({ hasText: /Suggested \(\d+\)/ });
  await expect(suggestionsDisclosure).toBeVisible();
  await expect(page.getByTestId("space-row-space-wenlan")).toBeVisible();
  await expect(page.getByTestId("space-row-space-suggested")).toBeHidden();
  await suggestionsDisclosure.click();
  await expect(page.getByTestId("space-row-space-suggested")).toBeVisible();

  await page.getByTestId("space-row-space-suggested").getByRole("button", { name: "AI Workflows" }).click();
  await page.getByRole("button", { name: "Keep", exact: true }).click();
  await expect(page.getByRole("button", { name: "Keep", exact: true })).toHaveCount(0);
  await page.getByRole("group", { name: "History navigation", exact: true }).getByRole("button", { name: "Back", exact: true }).click();

  // Returning to the overview remounts it with the disclosure collapsed again.
  await suggestionsDisclosure.click();
  await expect(page.getByTestId("space-row-space-suggested-2")).toBeVisible();
  await page.getByTestId("space-row-space-suggested-2").getByRole("button", { name: "Product Signals" }).click();
  await page.getByRole("button", { name: "Discard", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
  await expect(page.getByTestId("space-row-space-suggested-2")).toHaveCount(0);
  expect(controller.calls().filter(({ command }) => command === "confirm_space")).toHaveLength(1);
  expect(controller.calls().filter(({ command }) => command === "delete_space")).toHaveLength(1);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("saves Space title through its keyboard-editable inline field", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { locale: "en", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  await openSpace(page);

  const titleEditor = page.getByRole("textbox", { name: "Name", exact: true });
  await titleEditor.focus();
  await expect(titleEditor).toBeFocused();
  await titleEditor.fill("Wenlan Detail");
  await titleEditor.press("Enter");
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan Detail" })).toBeVisible();
  await expect(titleEditor).toHaveValue("Wenlan Detail");
  await expect.poll(() => controller.calls().filter(({ command }) => command === "update_space").length).toBe(1);
  const updates = controller.calls().filter(({ command }) => command === "update_space");
  expect(updates).toHaveLength(1);
  expect(updates[0]?.args).toEqual({
    description: "Editorial memory system",
    name: "Wenlan",
    newName: "Wenlan Detail",
  });
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
