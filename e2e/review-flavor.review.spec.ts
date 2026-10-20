// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { collectBrowserErrors } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";
import type { Page as KnowledgePage } from "../src/lib/tauri";

async function openWiki(page: Page): Promise<void> {
  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Wiki", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a note" })).toBeVisible();
}

async function openFixtureArchitecture(page: Page): Promise<void> {
  await openWiki(page);
  await openWikiNote(page, "Fixture architecture");
  await expect(
    page.getByRole("heading", { level: 1, name: "Fixture architecture" }),
  ).toBeVisible();
}

/** Read the same isolated runtime that answers the Review UI's IPC calls. */
async function storedReviewPage(page: Page, id: string): Promise<KnowledgePage | null> {
  return page.evaluate(async (pageId) => {
    const modulePath = "/review/tauri-core.ts";
    const { invoke } = await import(modulePath);
    return invoke("get_page", { id: pageId });
  }, id);
}

async function storedReviewPages(page: Page, status: "active" | "draft"): Promise<KnowledgePage[]> {
  return page.evaluate(async (pageStatus) => {
    const modulePath = "/review/tauri-core.ts";
    const { invoke } = await import(modulePath);
    return invoke("list_pages", { status: pageStatus, limit: 500, offset: 0 });
  }, status);
}

async function closeWritingView(page: Page): Promise<void> {
  const editor = page.getByRole("textbox", { name: "Note editor", exact: true });
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  // Escape flushes and returns to the reading surface. The editor may remain
  // mounted as part of the workspace, so assert its user-visible state.
  await editor.press("Escape");
  await expect(editor).not.toBeVisible();
  await expect(page.locator(".page-detail")).toBeVisible();
}

// The fixture library is small enough to show the getting-started card, which
// hides the Wiki directory these flows browse. Start each test with it dismissed.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("onboarding:gettingStartedDismissed", "1");
  });
});

async function installRejectedCommandAudit(page: Page): Promise<void> {
  await page.addInitScript(() => {
    Reflect.set(window, "__WENLAN_REVIEW_COMMAND_FAILURES__", []);
  });
}

async function rejectedCommands(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const failures = Reflect.get(window, "__WENLAN_REVIEW_COMMAND_FAILURES__");
    return Array.isArray(failures) ? failures : [];
  });
}

test("keeps every enabled primary destination inside the Review command contract", async ({
  page,
}) => {
  // This route-contract pass spans the whole navigation rail. Its first Vite
  // cold load can consume most of the default timeout before any route opens.
  test.setTimeout(60_000);
  const browserErrors = collectBrowserErrors(page);
  await installRejectedCommandAudit(page);
  await page.goto("/");

  const navigation = page.getByRole("navigation", { name: "Primary navigation" });

  await navigation.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a note" })).toBeVisible();

  await navigation.getByRole("button", { name: "Spaces", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toBeVisible();

  await navigation.getByRole("button", { name: "Graph", exact: true }).click();
  await expect(page.getByTestId("atlas-view")).toBeVisible();
  // Same fixture and same count line as graph-rendering.visual.spec: notes
  // lead, and the topic count is over what is drawn (the three connected ones).
  await expect(page.getByText(/^7 notes · 3 topics(?: · \d+ regions?)?$/)).toBeVisible();

  await openPrimaryDestination(page, "Memories");
  await expect(page.getByRole("region", { name: "Memory list" })).toBeVisible();
  // Keep the same fixture row even if a mutation changes list ordering.
  const firstMemory = page.getByRole("article", { name: "Fixture architecture", exact: true });
  const memoryActions = firstMemory.getByRole("button", { name: "Memory actions", exact: true });
  // The reading row reveals this control when it receives keyboard focus.
  // Focusing it first makes the interaction match the accessible user path.
  await memoryActions.focus();
  await expect(memoryActions).toBeFocused();
  await page.keyboard.press("Enter");
  await firstMemory.getByRole("menuitem", { name: "Unpin memory", exact: true }).click();
  await memoryActions.focus();
  await page.keyboard.press("Enter");
  await expect(firstMemory.getByRole("menuitem", { name: "Pin memory", exact: true })).toBeVisible();
  await firstMemory.getByRole("menuitem", { name: "Unconfirm memory", exact: true }).click();
  await memoryActions.focus();
  await page.keyboard.press("Enter");
  await expect(firstMemory.getByRole("menuitem", { name: "Confirm memory", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(memoryActions).toBeFocused();

  await openPrimaryDestination(page, "Sources");
  await expect(page.getByRole("heading", { level: 1, name: "Sources", exact: true })).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Bring your sources together", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("group", { name: "Filter sources", exact: true })).toBeVisible();
  await expect(page.getByRole("searchbox", { name: "Filter sources", exact: true })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Sources", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(navigation.getByRole("button", { name: "More", exact: true })).not.toHaveAttribute("aria-current");

  await openPrimaryDestination(page, "Wiki");
  await expect(page.getByRole("heading", { level: 1, name: "Open a note", exact: true })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Wiki", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(navigation.getByRole("button", { name: "More", exact: true })).not.toHaveAttribute("aria-current");

  await page.waitForTimeout(250);
  expect(await rejectedCommands(page)).toEqual([]);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("proves Review identity and exercises Wiki Page mutations", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await installRejectedCommandAudit(page);
  await page.goto("/");

  const fixtureNotice = page.locator('[data-review-environment="fixture-only"]');
  await expect(fixtureNotice).toBeVisible();
  await expect(fixtureNotice).toContainText("TEST DATA");
  await expect(fixtureNotice).toHaveAttribute("aria-label", "Fixture data · resets on relaunch");

  await openFixtureArchitecture(page);
  const initialPage = await storedReviewPage(page, "page-architecture");
  expect(initialPage).not.toBeNull();
  const editor = page.getByRole("textbox", { name: "Note editor", exact: true });
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  const editedSource = "# Fixture architecture\n\nEdited through the Review-flavor lane.";
  await editor.fill(editedSource);
  await expect.poll(async () => (await storedReviewPage(page, "page-architecture"))?.content).toBe(editedSource);
  const savedPage = await storedReviewPage(page, "page-architecture");
  expect(savedPage?.version).toBeGreaterThan(initialPage!.version);
  expect(savedPage?.user_edited).toBe(true);
  await expect(page.locator('.page-detail [role="status"]')).toHaveText("Saved");

  // Reopening uses persisted fixture content rather than the original editor DOM.
  await openFixtureArchitecture(page);
  await closeWritingView(page);
  await expect(page.getByText("Edited through the Review-flavor lane.")).toBeVisible();

  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Note actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Update from sources", exact: true }).click();
  await expect(page.getByText("Note updated from its sources.", { exact: true })).toBeVisible();
  await expect.poll(async () => (await storedReviewPage(page, "page-architecture"))?.last_compiled).toBe("2026-07-10T12:33:00Z");

  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Note actions" }).click();
  await page.getByRole("menuitem", { name: "Delete note" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a note" })).toBeVisible();
  await expect(
    page.locator(".wiki-workspace-directory").getByText("Fixture architecture", { exact: true }),
  ).toHaveCount(0);
  expect(await storedReviewPage(page, "page-architecture")).toBeNull();

  expect(await rejectedCommands(page)).toEqual([]);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("creates and finalizes a Page draft through the Review runtime", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await installRejectedCommandAudit(page);
  await page.goto("/");
  await openWiki(page);

  await page.locator(".note-tab-create").click();
  await page.getByRole("textbox", { name: "Title", exact: true }).fill(
    "Review lane authored Page",
  );
  const content = "This Page proves draft creation and finalization in the isolated Review runtime.";
  await page.getByRole("textbox", { name: "Content", exact: true }).fill(content);

  await expect.poll(async () => {
    const pages = await storedReviewPages(page, "draft");
    return pages.find((candidate) => candidate.title === "Review lane authored Page" && candidate.content === content) ?? null;
  }).not.toBeNull();
  const draft = (await storedReviewPages(page, "draft")).find(
    (candidate) => candidate.title === "Review lane authored Page" && candidate.content === content,
  );
  expect(draft).toBeDefined();
  expect(draft?.status).toBe("draft");
  const draftIdentity = draft!.id;
  const draftVersion = draft!.version;

  // Escape is the approved automatic finalization route; there is no Publish button.
  await page.getByRole("textbox", { name: "Content", exact: true }).press("Escape");
  await expect.poll(async () => (await storedReviewPage(page, draftIdentity))?.status).toBe("active");
  const finalized = await storedReviewPage(page, draftIdentity);
  expect(finalized).toMatchObject({
    id: draftIdentity,
    title: "Review lane authored Page",
    content,
    status: "active",
  });
  expect(finalized!.version).toBeGreaterThan(draftVersion);

  await openWiki(page);
  await openWikiNote(page, "Review lane authored Page");
  await expect(page.getByRole("heading", { level: 1, name: "Review lane authored Page" })).toBeVisible();
  await expect(page.getByText(content, { exact: true })).toBeVisible();
  expect(await rejectedCommands(page)).toEqual([]);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("marks a stored page reviewed through the Review presence contract", async ({ page }) => {
  // Review answers page_review_supported and review_page through its isolated
  // fixture runtime. This exercises the app gesture and registered command
  // contract; a real daemon's minted presence proof is a separate live lane.
  const browserErrors = collectBrowserErrors(page);
  await installRejectedCommandAudit(page);
  await page.goto("/");
  await openFixtureArchitecture(page);

  // Review is available while writing. The action flushes and attests the
  // exact stored editor source without closing the current editor session.
  const initialPage = await storedReviewPage(page, "page-architecture");
  expect(initialPage).not.toBeNull();
  const editor = page.getByRole("textbox", { name: "Note editor", exact: true });
  await expect(editor).toBeVisible();
  const reviewedSource = `${initialPage!.content}\n\nEdited in the Review editor before marking reviewed.`;
  await editor.fill(reviewedSource);
  // The Info action lives in the note sidebar, opened from the toolbar.
  await page.getByRole("button", { name: "Open note sidebar", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Info", exact: true })).toBeVisible();
  const review = page.getByRole("button", { name: "Mark note reviewed", exact: true });
  await expect(review).toBeEnabled();
  await review.click();

  await expect(page.getByTestId("page-review-notice")).toHaveText("Marked as reviewed.");
  await expect(editor).toBeVisible();
  await expect.poll(async () => (await storedReviewPage(page, "page-architecture"))?.content).toBe(reviewedSource);

  // Nothing was rejected by `review/tauri-core.ts`, which is the assertion the
  // contract exists to make.
  await page.waitForTimeout(250);
  expect(await rejectedCommands(page)).toEqual([]);
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("creates a Space and navigates to its rendered detail", async ({ page }) => {
  const browserErrors = collectBrowserErrors(page);
  await page.goto("/");

  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Spaces", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toBeVisible();

  await page.getByRole("button", { name: "New", exact: true }).click();
  await page.getByRole("textbox", { name: "Name", exact: true }).fill("Review Lane");
  await page.getByRole("textbox", { name: "Description", exact: true }).fill(
    "Created by the Review-flavor Playwright lane.",
  );
  await page.getByRole("button", { name: "Create", exact: true }).click();

  const card = page.getByTestId("space-card-space-review-lane");
  await expect(card.getByRole("button", { name: "Open Review Lane", exact: true })).toBeVisible();
  await card.getByRole("button", { name: "Open Review Lane", exact: true }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Review Lane" }),
  ).toBeVisible();
  await expect(
    page.getByRole("textbox", { name: "Edit description", exact: true }),
  ).toHaveValue("Created by the Review-flavor Playwright lane.");

  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
