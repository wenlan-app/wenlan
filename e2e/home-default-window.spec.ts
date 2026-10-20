// SPDX-License-Identifier: AGPL-3.0-only
//
// Wiki has to fit the window the app opens at. That size is not a
// guess: app/tauri.conf.json's main window is 1280x720 with the sidebar
// expanded, so this file measures geometry at exactly that viewport.
//
// Geometry, not pixels, on purpose: the approved snapshots in this suite are
// macOS captures and cannot be regenerated from a Windows or Linux checkout, so
// a screenshot assertion here would be unrunnable for most of the people who
// need it. Rects and scrollWidth are platform-independent.
import { expect, test, type Page as BrowserPage } from "@playwright/test";
import type { Page as KnowledgePage } from "../src/lib/tauri";
import {
  createSpacesNavigationFixture,
  type SpacesNavigationFixture,
} from "./fixtures/spacesNavigation";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

/** app/tauri.conf.json → app.windows[0].{width,height}. */
const DEFAULT_WINDOW = { width: 1280, height: 720 } as const;

/**
 * A library on first run: nothing has been captured, distilled or proposed yet,
 * so Wiki renders its empty state. Overriding only `pages` on the
 * populated fixture would leave 205 memories and a pending review queue behind
 * it, which is a different layout.
 */
function createFirstRunFixture(): SpacesNavigationFixture {
  const populated = createSpacesNavigationFixture();
  return {
    ...populated,
    pages: [] as readonly KnowledgePage[],
    memories: [],
    entities: [],
    entityDetails: [],
    refinements: [],
    distillReview: {
      ...populated.distillReview,
      pending: [],
      stale_pages: [],
      orphan_topics: [],
    },
  };
}

async function openWiki(page: BrowserPage, fixture: SpacesNavigationFixture, { gettingStarted = false } = {}) {
  const browserErrors = collectBrowserErrors(page);
  await installTauriMock(page, {
    fixture,
    gettingStarted,
    locale: "en",
    localStorage: { "wenlan-theme": "dark" },
    rawActions: [],
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1, name: "Open a page", exact: true })).toBeVisible();
  const wiki = page.locator(".wiki-overview");
  await expect(wiki).toBeVisible();
  await expect(page.locator(".note-tab-create")).toBeVisible();
  await expect(wiki.getByRole("table")).toHaveCount(0);
  await expect(wiki.locator(".asset-card, .wiki-page-link, .wiki-pagination")).toHaveCount(0);
  await expect(page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Wiki", exact: true })).toHaveAttribute("aria-current", "page");
  return browserErrors;
}

/**
 * Every element's right edge, against the viewport.
 *
 * Content inside a deliberately scrollable box is exempt: a wide table or code
 * block that scrolls within its own container is not a layout defect, and the
 * container itself is still measured on its own pass through this loop.
 */
async function horizontalOverflow(page: BrowserPage) {
  return page.evaluate(() => {
    const doc = document.documentElement;
    const inScrollContainer = (el: Element) => {
      for (let parent = el.parentElement; parent; parent = parent.parentElement) {
        const overflowX = getComputedStyle(parent).overflowX;
        if (overflowX === "auto" || overflowX === "scroll") return true;
      }
      return false;
    };
    const past: string[] = [];
    for (const el of Array.from(document.querySelectorAll("*"))) {
      const box = el.getBoundingClientRect();
      if (box.width === 0 && box.height === 0) continue;
      if (box.right <= window.innerWidth + 0.5) continue;
      if (inScrollContainer(el)) continue;
      const testid = el.getAttribute("data-testid");
      past.push(`${el.tagName.toLowerCase()}${testid ? `[${testid}]` : ""} right=${Math.round(box.right)}`);
    }
    return { documentOverflow: doc.scrollWidth - doc.clientWidth, past };
  });
}

const LIBRARIES: readonly (readonly [string, () => SpacesNavigationFixture])[] = [
  ["a first-run library", createFirstRunFixture],
  ["a library with pages", createSpacesNavigationFixture],
];

for (const [label, makeFixture] of LIBRARIES) {
  test(`Wiki fits the default window with ${label}`, async ({ page }) => {
    await page.setViewportSize({ ...DEFAULT_WINDOW });
    const browserErrors = await openWiki(page, makeFixture());

    const { documentOverflow, past } = await horizontalOverflow(page);
    expect(past, "no element may extend past the right edge of the default window").toEqual([]);
    expect(documentOverflow, "the document must not scroll horizontally").toBe(0);

    expect(browserErrors.pageErrors).toEqual([]);
    expect(browserErrors.consoleErrors).toEqual([]);
  });
}

test("a new user's home keeps one column and fits 1280x800 while getting started is open", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const populated = createSpacesNavigationFixture();
  await openWiki(page, { ...populated, pages: populated.pages.slice(0, 2) }, { gettingStarted: true });

  const checklist = page.getByTestId("getting-started");
  await expect(checklist).toBeVisible();
  await expect(page.locator(".wiki-reading-start")).toHaveAttribute("data-onboarding", "true");
  await expect(page.locator(".wiki-workspace")).toHaveAttribute("data-directory-hidden", "true");
  await expect(page.locator(".wiki-workspace-directory")).toBeHidden();
  const overflow = await page.evaluate(() => {
    const scroller = document.querySelector(".note-group-content") ?? document.documentElement;
    return {
      document: document.documentElement.scrollHeight - document.documentElement.clientHeight,
      content: scroller.scrollHeight - scroller.clientHeight,
    };
  });
  expect(overflow.document).toBeLessThanOrEqual(0);
  expect(overflow.content).toBeLessThanOrEqual(0);

  // Adding a source opens its dialog without leaving the Wiki.
  await checklist.getByRole("button", { name: "Add a source" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);

  // Hiding the card brings the directory back.
  await checklist.getByRole("button", { name: "Hide getting started" }).click();
  await expect(page.locator(".wiki-workspace-directory")).toBeVisible();
  await expect(page.locator(".wiki-reading-start")).not.toHaveAttribute("data-onboarding");
});

test("empty pages can start writing without AI setup", async ({ page }) => {
  await page.setViewportSize({ ...DEFAULT_WINDOW });
  await openWiki(page, createFirstRunFixture());
  await expect(page.locator("[data-ghost-card]")).toHaveCount(0);
  await expect(page.locator(".wiki-overview").getByRole("heading", { name: "Open a page", exact: true })).toBeVisible();
  await page.locator(".note-tab-create").click();
  await expect(page.getByRole("textbox", { name: "Content", exact: true })).toBeVisible();
});

test("the home button returns to the Wiki home while the open page keeps its tab", async ({ page }) => {
  await page.setViewportSize({ ...DEFAULT_WINDOW });
  await openWiki(page, createFirstRunFixture());
  const home = page.getByRole("button", { name: "Wiki home", exact: true });
  const overviewTitle = page.locator(".wiki-overview").getByRole("heading", { name: "Open a page", exact: true });
  // On the home the button has nowhere to go, so it is not shown.
  await expect(overviewTitle).toBeVisible();
  await expect(home).toBeHidden();

  await page.locator(".note-tab-create").click();
  await expect(page.getByRole("textbox", { name: "Content", exact: true })).toBeVisible();
  await expect(home).toBeVisible();
  await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(1);

  await home.click();
  await expect(overviewTitle).toBeVisible();
  await expect(home).toBeHidden();
  await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(1);
});

// Review page changes is an existing Space dossier action, and Back returns
// to that Space. This test does not add a new Activity-to-Review entry point.
test("the Space review action opens Review and Back returns to the Space", async ({ page }) => {
  await page.setViewportSize({ ...DEFAULT_WINDOW });
  const browserErrors = await openWiki(page, createSpacesNavigationFixture());
  await expect(page.getByTestId("wiki-page-updates")).toHaveCount(0);
  await expect(page.getByTestId("wiki-context-rail")).toHaveCount(0);

  const primaryNavigation = page.getByRole("navigation", { name: "Primary navigation" });
  await primaryNavigation.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();

  await expect(page.getByRole("button", { name: "Review page changes" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Page options", exact: true })).toHaveCount(0);
  await primaryNavigation.getByRole("button", { name: "Spaces", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces", exact: true })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
  await page.getByRole("button", { name: "Open Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Actions for Wenlan", exact: true }).click();
  await page.getByRole("menuitem", { name: "Review page changes", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Review", exact: true })).toBeVisible();
  // Review stays on the Wiki navigation surface. Back returns to the Space
  // dossier where the existing action lives.
  await expect(primaryNavigation.getByRole("button", { name: "Wiki", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);

  const historyBack = page
    .getByRole("group", { name: "History navigation" })
    .getByRole("button", { name: "Back", exact: true });
  await historyBack.click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan", exact: true })).toBeVisible();

  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

// PR4 Activity-to-Review navigation remains alongside PR3's Space action path.
// Review is opt-in from Activity; Back returns to the activity page.
test("review opens from Activity and Back returns to Activity", async ({ page }) => {
  await page.setViewportSize({ ...DEFAULT_WINDOW });
  const browserErrors = await openWiki(page, createSpacesNavigationFixture());
  await expect(page.getByTestId("wiki-page-updates")).toHaveCount(0);
  await expect(page.getByTestId("wiki-context-rail")).toHaveCount(0);

  const primaryNavigation = page.getByRole("navigation", { name: "Primary navigation" });
  await primaryNavigation.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();

  await expect(page.getByRole("button", { name: "Review page changes" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Page options", exact: true })).toHaveCount(0);
  await page.getByTestId("activity-status").click();
  await page.getByTestId("activity-summary-open").click();
  await expect(page.getByRole("heading", { level: 1, name: "Activity" })).toBeVisible();
  const reviewEntry = page.getByRole("button", { name: "Review page changes", exact: true });
  await reviewEntry.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { level: 1, name: "Review" })).toBeVisible();
  // The review is still scoped to the note library; navigation history remembers Activity.
  await expect(primaryNavigation.getByRole("button", { name: "Wiki", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(primaryNavigation.locator('[aria-current="page"]')).toHaveCount(1);

  await page.getByRole("button", { name: "Back" }).first().click();
  await expect(page.getByRole("heading", { level: 1, name: "Activity" })).toBeVisible();

  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
