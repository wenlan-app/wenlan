// SPDX-License-Identifier: AGPL-3.0-only
// Activity rail and compact summary behavior (PR3), plus the ActivityNow
// neutral, failure, and editor-preservation contracts (PR4).
import { expect, test, type Page } from "@playwright/test";
import type { ActivityResponse } from "../src/lib/tauri";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

const WIDTHS = [1280, 375] as const;
const VIEWS = ["Wiki", "Topics", "Spaces", "Graph", "Sources"] as const;

/**
 * A fixture with work in every asset, so the popover has three non-empty
 * rows. Store, Confirm, and Detect have different units; the count and bar
 * assertions below keep those distinctions visible.
 */
const ACTIVITY_FIXTURE: ActivityResponse = {
  state: "organizing",
  last_activity_at: 1_783_728_000,
  assets: [
    {
      kind: "memories",
      state: "running",
      done: 7,
      total: 12,
      blocked: 0,
      steps: [
        { name: "store", state: "idle", done: 12, total: 12, failed: 0, job: null },
        {
          name: "summarize",
          state: "running",
          done: 7,
          total: 12,
          failed: 0,
          job: "everyday",
        },
        { name: "link", state: "idle", done: 7, total: 12, failed: 0, job: "everyday" },
      ],
    },
    {
      kind: "entities",
      state: "idle",
      done: 9,
      total: 21,
      blocked: 0,
      steps: [
        { name: "detect", state: "idle", done: 12, total: 12, failed: 0, job: "everyday" },
        { name: "confirm", state: "idle", done: 9, total: 21, failed: 0, job: null },
      ],
    },
    {
      kind: "pages",
      state: "idle",
      done: 3,
      total: 3,
      blocked: 0,
      steps: [
        { name: "write", state: "idle", done: 3, total: 3, failed: 0, job: "synthesis" },
      ],
    },
  ],
  everyday: {
    job: "everyday",
    lane: "on_device",
    model: "Qwen3 4B",
    mode: "auto",
    available: true,
  },
  synthesis: {
    job: "synthesis",
    lane: "anthropic",
    model: "Claude Sonnet",
    mode: "pinned",
    available: true,
  },
  refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
};

async function installActivityFixture(page: Page): Promise<void> {
  await page.addInitScript((fixture) => {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) return;
    const orig = internals.invoke.bind(internals);
    internals.invoke = (async (command: string, args?: unknown) => {
      if (command === "get_activity") return fixture;
      return orig(command, args);
    }) as typeof internals.invoke;
  }, ACTIVITY_FIXTURE);
}

async function openSidebar(page: Page): Promise<void> {
  const aside = page.locator("aside").first();
  await expect(aside).toBeAttached();
  if ((await aside.getAttribute("aria-hidden")) === "true") {
    await page.locator('[data-sidebar-toggle="true"]').click();
    await expect(aside).toHaveAttribute("aria-hidden", "false");
  }
}

for (const width of WIDTHS) {
  test(`Activity rail remains reachable across views at ${width}px`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
    await installActivityFixture(page);
    await page.goto("/");

    for (const view of VIEWS) {
      await openSidebar(page);
      await openPrimaryDestination(page, view);
      // Navigation closes the overlay drawer at narrow widths; reopen it to
      // verify the rail's actual mobile render path rather than a hidden node.
      await openSidebar(page);

      const button = page.getByTestId("activity-status");
      await expect(button).toBeVisible();
      await expect(button).toHaveAttribute("data-state", "organizing");
      await expect(button).toHaveAccessibleName("Activity, Steeping");
      await expect(page.getByTestId("activity-status-icon")).toHaveAttribute(
        "data-icon-state", "organizing",
      );
      await expect(page.getByTestId("activity-status-icon")).toHaveClass(/mem-activity-pulse-running/);

      const placement = () => button.evaluate((node) => {
        const box = node.getBoundingClientRect();
        return {
          inHeader: node.closest("header") !== null,
          inSidebar: node.closest("aside") !== null,
          onScreen: box.left >= 0 && box.right <= window.innerWidth && box.top >= 0 && box.bottom <= window.innerHeight,
        };
      });
      await expect.poll(placement, { message: `status placement on ${view}` }).toEqual({
        inHeader: false, inSidebar: true, onScreen: true,
      });
      await expect(button).toHaveCount(1);

      const documentOverflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(documentOverflow, `document overflow on ${view}`).toBeLessThanOrEqual(1);
    }

    // Activity owns its footer selection; the primary navigation has no
    // selected destination when its dedicated view is open.
    const trigger = page.getByTestId("activity-status");
    await trigger.click();
    const popover = page.getByRole("dialog", { name: "Background activity" });
    await expect(popover).toBeVisible();
    await page.getByTestId("activity-summary-open").click();
    await expect(page.getByTestId("activity-now")).toBeVisible();
    await openSidebar(page);
    const primaryNavigation = page.getByRole("navigation", { name: "Primary navigation" });
    await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(0);
    await expect(trigger).toHaveAttribute("aria-current", "page");

    // The rail popover opens upward and stays within the viewport at every
    // width, including the overlay drawer on phone-sized windows.
    await trigger.click();
    await expect(popover).toBeVisible();
    const box = await popover.evaluate((node) => {
      const rect = node.getBoundingClientRect();
      const triggerRect = document.querySelector('[data-testid="activity-status"]')!.getBoundingClientRect();
      return { left: rect.left, right: rect.right, bottom: rect.bottom, triggerTop: triggerRect.top };
    });
    expect(box.left, "popover left edge").toBeGreaterThanOrEqual(0);
    expect(box.right, "popover right edge").toBeLessThanOrEqual(width);
    expect(box.bottom, "popover above the rail trigger").toBeLessThanOrEqual(box.triggerTop);
    await page.keyboard.press("Escape");
    await expect(popover).toBeHidden();
    await expect(trigger).toBeFocused();

    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}

test("Activity summary preserves per-asset counts, keyboard focus, and navigation", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
  await installActivityFixture(page);
  await page.goto("/");

  const trigger = page.getByTestId("activity-status");
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  await trigger.click();
  const popover = page.getByRole("dialog", { name: "Background activity" });
  await expect(popover).toBeVisible();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");

  // Counts use each row's own unit. The entities row counts confirmed entities,
  // while its progress bar tracks memories scanned for entities.
  await expect(popover.getByTestId("activity-asset-count-memories")).toHaveText("12");
  await expect(popover.getByTestId("activity-asset-count-entities")).toHaveText("21");
  await expect(popover.getByTestId("activity-asset-count-pages")).toHaveText("3");
  await expect(popover.getByTestId("activity-asset-bar-memories")).toHaveAttribute(
    "data-fraction", String(7 / 12),
  );
  await expect(popover.getByTestId("activity-asset-bar-entities")).toHaveAttribute("data-fraction", "1");
  await expect(popover.getByTestId("activity-asset-bar-pages")).toHaveAttribute("data-fraction", "1");

  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  await expect(trigger).toBeFocused();

  await page.keyboard.press("Enter");
  await expect(popover).toBeVisible();
  await page.getByTestId("activity-summary-open").click();
  await expect(popover).toBeHidden();
  await expect(page.getByTestId("activity-now")).toBeVisible();
  const primaryNavigation = page.getByRole("navigation", { name: "Primary navigation" });
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(0);
  await expect(trigger).toHaveAttribute("aria-current", "page");

  // Outside pointer input closes the summary without trapping focus.
  await trigger.click();
  await expect(popover).toBeVisible();
  await page.mouse.click(640, 700);
  await expect(popover).toBeHidden();

  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("Activity summary is reachable by keyboard alone", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
  await installActivityFixture(page);
  await page.goto("/");

  const trigger = page.getByTestId("activity-status");
  await trigger.focus();
  await expect(trigger).toBeFocused();
  await page.keyboard.press("Enter");

  const popover = page.getByRole("dialog", { name: "Background activity" });
  await expect(popover).toBeVisible();
  await expect(popover).toHaveAttribute("aria-label", "Background activity");
  await expect(popover.getByRole("button", { name: "See all activity" })).toBeVisible();
  // Decorative row swatches must stay out of the accessibility tree; if a
  // future icon/image is meaningful, it needs an accessible label instead.
  const unlabelledImages = await popover.evaluate((node) =>
    [...node.querySelectorAll("svg, img")].filter(
      (element) =>
        element.getAttribute("aria-hidden") !== "true" &&
        !element.getAttribute("aria-label") &&
        !element.getAttribute("alt"),
    ).length,
  );
  expect(unlabelledImages).toBe(0);
  await page.keyboard.press("Tab");
  await expect(popover.getByTestId("activity-summary-open")).toBeFocused();
});

// PR4 ActivityFeed and ActivityNow contracts, kept separate from the rail fixture above.
const IDLE_ACTIVITY: ActivityResponse = {
  state: "blocked",
  last_activity_at: null,
  assets: [],
  everyday: { job: "everyday", lane: "none", model: null, mode: "unconfigured", available: false },
  synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
  refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
};

async function installPr4ActivityFixture(page: Page, activity: ActivityResponse): Promise<void> {
  await page.addInitScript((fixture) => {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) return;
    const orig = internals.invoke.bind(internals);
    internals.invoke = (async (command: string, args?: unknown) => {
      if (command === "get_activity") return fixture;
      return orig(command, args);
    }) as typeof internals.invoke;
  }, activity);
}

test("Activity status stays neutral without configured AI and keeps Activity sections visible", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
  await installPr4ActivityFixture(page, IDLE_ACTIVITY);
  await page.goto("/");

  const button = page.getByTestId("activity-status");
  await expect(button).toBeVisible();
  await expect(button).toHaveAccessibleName("Activity");
  await expect(button).not.toHaveAttribute("data-state");
  await expect(page.getByTestId("activity-status-icon")).toHaveAttribute("data-icon-kind", "pulse");
  await button.click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.getByTestId("activity-summary-open").click();

  await expect(page.getByRole("heading", { name: "Activity", level: 1 })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Background organization", level: 2 })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Suggestions", level: 2 })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Recent activity", level: 2 })).toBeVisible();
  await expect(page.getByTestId("activity-now-blocked")).toBeVisible();
  await expect(page.getByTestId("activity-now-idle")).toHaveCount(0);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByText(/turn on model|choose a model|configure a model/i)).toHaveCount(0);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("a failed step is the only attention state on the Activity entry", async ({ page }) => {
  await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
  await installPr4ActivityFixture(page, {
    ...IDLE_ACTIVITY,
    assets: [{
      kind: "pages",
      state: "blocked",
      done: 0,
      total: 2,
      blocked: 2,
      steps: [{ name: "write", state: "blocked", done: 0, total: 2, failed: 2, job: "synthesis" }],
    }],
  });
  await page.goto("/");

  const button = page.getByTestId("activity-status");
  await expect(button).toHaveAccessibleName("Activity, Failed");
  await expect(page.getByTestId("activity-status-icon")).toHaveAttribute("data-icon-kind", "attention");
  await button.click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.getByTestId("activity-summary-open").click();
  await expect(page.getByTestId("activity-now-failed")).toHaveText("2 pages could not be updated.");
});


test("summary Escape keeps the page editor open and returns focus to Activity", async ({ page }) => {
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await installPr4ActivityFixture(page, IDLE_ACTIVITY);
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  const editor = page.locator(".cm-content[contenteditable=true]");
  await expect(editor).toBeVisible();
  await editor.evaluate(el => el.setAttribute("data-summary-original", "true"));
  const text = await editor.innerText();
  const trigger = page.getByTestId("activity-status");
  await trigger.click();
  await page.getByTestId("activity-summary-open").focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();
  await expect(trigger).toBeFocused();
  await expect(editor).toHaveAttribute("data-summary-original", "true");
  expect(await editor.innerText()).toBe(text);
});
