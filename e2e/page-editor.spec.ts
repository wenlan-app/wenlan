// SPDX-License-Identifier: AGPL-3.0-only
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { expect, test, type Locator, type Page } from "@playwright/test";
import type {
  Page as WenlanPage,
  UpdatePageInput,
  UpdatePageOutcome,
} from "../src/lib/tauri";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import {
  installTauriMock,
  type TauriMockController,
  type TauriMockPageScenario,
} from "./tauriMock";

const PAGE_ID = "page-editor-e2e";
const PAGE_TITLE = "Browser editor fixture";
const INITIAL_SOURCE = "# Browser editor fixture\n\nInitial source paragraph.\n";
const UUID_V4 =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
// Comfortably past the 650ms autosave debounce, used only to prove a write
// did NOT happen; positive expectations poll instead.
const QUIET_WINDOW_MS = 1_500;
const AUTOSAVE_TIMEOUT = { timeout: 10_000 };
// Canonical readback after a saved CAS write. The contract reads with
// explicit intent (`get_page_explicit_browse`); plain `get_page` is accepted
// so the assertion pins the ordering, not the wrapper choice.
const PAGE_READ_COMMANDS = new Set(["get_page", "get_page_explicit_browse"]);

function pageFixture(content = INITIAL_SOURCE): WenlanPage {
  return {
    id: PAGE_ID,
    title: PAGE_TITLE,
    summary: null,
    content,
    entity_id: null,
    domain: "Wenlan",
    space: "Wenlan",
    source_memory_ids: [],
    version: 7,
    status: "active",
    creation_kind: "authored",
    review_status: "confirmed",
    created_at: "2026-07-19T12:00:00.000Z",
    last_compiled: "2026-07-19T12:00:00.000Z",
    last_modified: "2026-07-19T12:00:00.000Z",
    user_edited: false,
  };
}

async function openFromWiki(page: Page, title = PAGE_TITLE): Promise<void> {
  await openWikiNote(page, title);
  await expect(page.locator(".page-detail")).toBeVisible();
}

async function openPage(
  page: Page,
  fixture = pageFixture(),
  pageScenario?: TauriMockPageScenario,
  delays?: Readonly<Record<string, number>>,
): Promise<TauriMockController> {
  const defaults = createSpacesNavigationFixture();
  const controller = await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    fixture: { ...defaults, pages: [fixture] },
    pageScenario,
    delays,
  });
  await page.goto("/");
  await openPrimaryDestination(page, "Wiki");
  await openFromWiki(page, fixture.title);
  return controller;
}

/**
 * An ordinary page opens straight into the editor: no Edit click. Waits for
 * the editor to hold the expected source so tests never race the lazy
 * CodeMirror module (or its fallback) while it is still mounting.
 */
async function openedEditor(page: Page, source = INITIAL_SOURCE): Promise<Locator> {
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  await expect.poll(() => editorSource(editor)).toBe(source);
  return editor;
}

async function editorSource(editor: Locator): Promise<string> {
  return editor.evaluate((element) => {
    if (element instanceof HTMLTextAreaElement) return element.value;
    type CodeMirrorContent = HTMLElement & {
      cmTile?: {
        root?: {
          view?: {
            state: { doc: { toString(): string } };
          };
        };
      };
    };
    const content = (
      element.classList.contains("cm-content")
        ? element
        : element.querySelector(".cm-content")
    ) as CodeMirrorContent | null;
    const view = content?.cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    return view.state.doc.toString();
  });
}

/** Tags the live editor node so a later remount is observable. */
async function markEditorSession(editor: Locator): Promise<void> {
  await editor.evaluate((element) => {
    (element as HTMLElement & { __e2eEditorSession?: boolean }).__e2eEditorSession = true;
  });
}

async function isSameEditorSession(editor: Locator): Promise<boolean> {
  return editor.evaluate((element) =>
    (element as HTMLElement & { __e2eEditorSession?: boolean }).__e2eEditorSession === true
  );
}

function updateCalls(controller: TauriMockController): UpdatePageInput[] {
  return controller.calls()
    .filter((call) => call.command === "update_page")
    .map((call) => call.args as UpdatePageInput);
}

/** Page reads issued after the nth update_page call and before the next one. */
function canonicalReadsAfterUpdate(
  controller: TauriMockController,
  updateNumber: number,
): number {
  let updatesSeen = 0;
  let reads = 0;
  for (const call of controller.calls()) {
    if (call.command === "update_page") {
      updatesSeen += 1;
      continue;
    }
    if (
      updatesSeen === updateNumber
      && PAGE_READ_COMMANDS.has(call.command)
      && (call.args as { id?: unknown } | undefined)?.id === PAGE_ID
    ) {
      reads += 1;
    }
  }
  return reads;
}

/**
 * Reads the mock store directly. This issues a `get_page` call, so check
 * canonical-read evidence with {@link canonicalReadsAfterUpdate} first.
 */
async function storedPage(page: Page): Promise<WenlanPage> {
  return await page.evaluate(async (id) =>
    window.__TAURI_INTERNALS__!.invoke("get_page", { id })
  , PAGE_ID) as WenlanPage;
}

function persistenceButtons(page: Page): Locator[] {
  const detail = page.locator(".page-detail");
  return [
    detail.getByRole("button", { name: "Save", exact: true }),
    detail.getByRole("button", { name: "Cancel", exact: true }),
  ];
}

test("opens an ordinary page directly in one editable writing view", async ({ page }) => {
  const controller = await openPage(page);

  const editor = await openedEditor(page);

  const saveStatus = page.locator('.page-detail [role="status"]');
  await expect(saveStatus).toHaveText("Saved");
  // Still announced to assistive technology, without a row above the title.
  await expect(saveStatus).toHaveCSS("position", "absolute");
  await expect(saveStatus).toHaveCSS("clip-path", "inset(50%)");
  await expect(page.getByRole("button", { name: "Page info", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Mind map", exact: true })).toHaveCount(0);
  const pageActions = page.getByRole("button", { name: "Page actions", exact: true });
  await expect(pageActions).toHaveText("");
  await expect(pageActions.locator("svg")).toHaveCount(1);

  for (const button of persistenceButtons(page)) await expect(button).toHaveCount(0);
  await expect(
    page.getByRole("radio", { name: "Live Preview", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("radio", { name: "Source mode", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.locator('[data-editor-presentation-mode="writing"]'),
  ).toBeVisible();

  const heading = editor.locator(".cm-line").first();
  await editor.locator(".cm-line").last().click();
  await expect(heading).toHaveText(PAGE_TITLE);
  await heading.click();
  await expect(heading).toHaveText("# Browser editor fixture");

  // Opening, focusing and moving the caret are not edits.
  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toEqual([]);
});

test("formats with a keyboard shortcut and toggles a task, then autosaves the exact source", async ({ page }) => {
  const source = "alpha\n\n- [ ] verify exact source";
  const formatted = "**alpha**\n\n- [x] verify exact source";
  const controller = await openPage(page, pageFixture(source));

  const editor = await openedEditor(page, source);
  const task = editor.locator(".cm-writing-task-checkbox");
  await expect(task).not.toBeChecked();
  await expect(task).toHaveAccessibleName("verify exact source");
  await task.focus();
  await task.press("Space");
  await expect(task).toBeChecked();
  await expect.poll(() => editorSource(editor)).toBe(
    "alpha\n\n- [x] verify exact source",
  );
  await expect(task).toBeFocused();

  await editor.locator(".cm-line").first().click();
  await editor.press("Home");
  await editor.press("Shift+End");
  await editor.press("ControlOrMeta+b");

  await expect(editor).toBeFocused();
  await expect.poll(() => editorSource(editor)).toBe(formatted);

  // Format commands are edits: the final source reaches the store verbatim.
  await expect.poll(
    () => updateCalls(controller).at(-1)?.content,
    AUTOSAVE_TIMEOUT,
  ).toBe(formatted);
  await expect.poll(async () => (await storedPage(page)).content, AUTOSAVE_TIMEOUT)
    .toBe(formatted);
  await expect.poll(() => editorSource(editor)).toBe(formatted);
});

test("autosaves the exact source through one typed CAS request and a canonical readback", async ({ page }) => {
  const controller = await openPage(page);
  const savedSource =
    "# Browser editor fixture\n\nSaved through the browser route exactly.  \n";
  const editor = await openedEditor(page);
  await markEditorSession(editor);

  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toEqual([]);

  await editor.fill(savedSource);
  await expect.poll(() => editorSource(editor)).toBe(savedSource);

  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(1);
  const [request] = updateCalls(controller);
  expect(request).toMatchObject({
    id: PAGE_ID,
    content: savedSource,
    expectedVersion: 7,
    callerId: "wenlan-app",
  } satisfies Omit<UpdatePageInput, "operationId">);
  expect(request?.operationId).toMatch(UUID_V4);
  await expect.poll(
    () => canonicalReadsAfterUpdate(controller, 1),
    AUTOSAVE_TIMEOUT,
  ).toBeGreaterThan(0);

  expect(await storedPage(page)).toMatchObject({
    id: PAGE_ID,
    content: savedSource,
    version: 8,
    user_edited: true,
  });

  // Same editor session, still editable, and the save is not repeated.
  await expect(editor).toBeVisible();
  await expect(editor).toBeEditable();
  expect(await isSameEditorSession(editor)).toBe(true);
  expect(await editorSource(editor)).toBe(savedSource);
  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toHaveLength(1);
});

test("chains a second edit on the confirmed canonical version", async ({ page }) => {
  const controller = await openPage(page);
  const firstSource = "# Browser editor fixture\n\nFirst autosaved edit.\n";
  const secondSource = "# Browser editor fixture\n\nSecond autosaved edit.\n";
  const editor = await openedEditor(page);
  await markEditorSession(editor);

  await editor.fill(firstSource);
  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(1);
  await expect.poll(
    () => canonicalReadsAfterUpdate(controller, 1),
    AUTOSAVE_TIMEOUT,
  ).toBeGreaterThan(0);
  expect(await storedPage(page)).toMatchObject({ content: firstSource, version: 8 });

  await editor.fill(secondSource);
  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(2);
  const [first, second] = updateCalls(controller);
  expect(first).toMatchObject({ content: firstSource, expectedVersion: 7 });
  expect(second).toMatchObject({
    id: PAGE_ID,
    content: secondSource,
    expectedVersion: 8,
    callerId: "wenlan-app",
  });
  expect(second?.operationId).toMatch(UUID_V4);
  expect(second?.operationId).not.toBe(first?.operationId);
  await expect.poll(
    () => canonicalReadsAfterUpdate(controller, 2),
    AUTOSAVE_TIMEOUT,
  ).toBeGreaterThan(0);

  expect(await storedPage(page)).toMatchObject({ content: secondSource, version: 9 });
  expect(await isSameEditorSession(editor)).toBe(true);
  expect(await editorSource(editor)).toBe(secondSource);
});

test("keeps typing while a save is in flight and saves the newest source on the new baseline", async ({ page }) => {
  const controller = await openPage(page, pageFixture(), undefined, {
    update_page: 1_200,
  });
  const pendingSource = "# Browser editor fixture\n\nSnapshot already in flight.\n";
  const newestSource =
    "# Browser editor fixture\n\nTyped while the first save was pending.\n";
  const editor = await openedEditor(page);
  await markEditorSession(editor);

  await editor.fill(pendingSource);
  // The mock logs the call before holding it, so the write is now in flight.
  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(1);
  await editor.fill(newestSource);
  await expect.poll(() => editorSource(editor)).toBe(newestSource);

  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(2);
  const [first, second] = updateCalls(controller);
  expect(first).toMatchObject({ content: pendingSource, expectedVersion: 7 });
  expect(second).toMatchObject({ content: newestSource, expectedVersion: 8 });
  expect(second?.operationId).not.toBe(first?.operationId);

  // The canonical readback of the first snapshot never replaced newer text.
  expect(await editorSource(editor)).toBe(newestSource);
  expect(await isSameEditorSession(editor)).toBe(true);
  await expect.poll(
    () => canonicalReadsAfterUpdate(controller, 2),
    AUTOSAVE_TIMEOUT,
  ).toBeGreaterThan(0);
  expect(await storedPage(page)).toMatchObject({ content: newestSource, version: 9 });
  expect(await editorSource(editor)).toBe(newestSource);
  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toHaveLength(2);
});

test("flushes the newest draft before sidebar navigation leaves the page", async ({ page }) => {
  const controller = await openPage(page, pageFixture(), undefined, {
    update_page: 600,
  });
  const draft = "# Browser editor fixture\n\nFlushed before leaving the page.  \n";
  const editor = await openedEditor(page);

  await editor.fill(draft);
  await openPrimaryDestination(page, "Wiki");

  await expect(page.locator(".page-detail")).toHaveCount(0, AUTOSAVE_TIMEOUT);
  // Read at once: a navigate-first-then-save bug still has the delayed write
  // in flight here and would show version 7.
  expect(await storedPage(page)).toMatchObject({ content: draft, version: 8 });
  await expect(page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Wiki", exact: true })).toHaveAttribute("aria-current", "page");
  expect(updateCalls(controller)).toHaveLength(1);
  expect(updateCalls(controller)[0]).toMatchObject({
    content: draft,
    expectedVersion: 7,
  });

  await openFromWiki(page);
  await openedEditor(page, draft);
});

test("stateful mock replays exact receipts and rejects reused or stale writes", async ({ page }) => {
  await openPage(page);
  await openedEditor(page);

  const outcomes = await page.evaluate(async (pageId) => {
    const invoke = window.__TAURI_INTERNALS__!.invoke;
    const request: UpdatePageInput = {
      id: pageId,
      content: "# Browser editor fixture\n\nFirst exact write.  \n",
      expectedVersion: 7,
      callerId: "wenlan-app",
      operationId: "11111111-1111-4111-8111-111111111111",
    };
    return {
      first: await invoke("update_page", request),
      replay: await invoke("update_page", request),
      changedReplay: await invoke("update_page", {
        ...request,
        content: `${request.content}changed`,
      }),
      stale: await invoke("update_page", {
        ...request,
        operationId: "22222222-2222-4222-8222-222222222222",
      }),
      page: await invoke("get_page", { id: pageId }),
    };
  }, PAGE_ID);

  expect(outcomes).toMatchObject({
    first: { outcome: "saved" },
    replay: { outcome: "saved" },
    changedReplay: {
      outcome: "conflict",
      message: "operation identity was already used for different page content",
    },
    stale: {
      outcome: "conflict",
      message: "expected version 7; current version is 8",
    },
    page: {
      id: PAGE_ID,
      content: "# Browser editor fixture\n\nFirst exact write.  \n",
      version: 8,
    },
  } satisfies {
    first: UpdatePageOutcome;
    replay: UpdatePageOutcome;
    changedReplay: UpdatePageOutcome;
    stale: UpdatePageOutcome;
    page: Partial<WenlanPage>;
  });
});

test("keeps the local draft and stops autosaving when a remote write wins the CAS race", async ({ page }) => {
  const remoteSource = "# Browser editor fixture\n\nRemote writer won.\n";
  const controller = await openPage(page, pageFixture(), {
    firstWriteRemoteMutation: { pageId: PAGE_ID, content: remoteSource },
  });
  const editor = await openedEditor(page);
  const localDraft = "# Browser editor fixture\n\nMy unsaved local draft.\n";

  await editor.fill(localDraft);

  const conflict = page.getByRole("alert");
  await expect(conflict).toContainText("This page changed elsewhere.", AUTOSAVE_TIMEOUT);
  await expect(conflict).toContainText("Your local draft is still here.");
  await expect(page.getByText("Latest source (version 8)")).toBeVisible();
  await expect.poll(() => editorSource(editor)).toBe(localDraft);
  expect(updateCalls(controller)).toHaveLength(1);
  expect(updateCalls(controller)[0]).toMatchObject({
    content: localDraft,
    expectedVersion: 7,
  });

  // More typing neither clears the conflict nor overwrites the remote copy.
  const extendedDraft = `${localDraft}Still typing after the conflict.\n`;
  await editor.fill(extendedDraft);
  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toHaveLength(1);
  await expect(conflict).toContainText("This page changed elsewhere.");
  expect(await editorSource(editor)).toBe(extendedDraft);

  const stored = await storedPage(page);
  expect(stored.content).toBe(remoteSource);
  expect(stored.version).toBe(8);
});

test("blocks editing below the daemon floor and records the typed diagnostic", async ({ page }) => {
  const controller = await openPage(page, pageFixture(), {
    daemonVersion: "0.14.0",
  });

  await expect(page.getByRole("alert")).toContainText(
    "Page editing requires stable Wenlan daemon 0.14.1 or later. Running version: 0.14.0.",
  );
  await expect(page.getByRole("textbox", { name: "Page editor" })).toHaveCount(0);
  await expect.poll(() =>
    controller.calls().filter(
      (call) => call.command === "record_page_editor_diagnostic",
    ).map((call) => call.args)
  ).toEqual([{
    event: "daemon_floor_blocked",
    reportedVersion: "0.14.0",
    requiredFloor: "0.14.1",
  }]);
  expect(updateCalls(controller)).toEqual([]);
});

test("keeps the local draft when the daemon falls below the save floor", async ({ page }) => {
  const controller = await openPage(page, pageFixture(), {
    daemonVersion: "0.14.1",
    saveDaemonVersion: "0.14.0",
  });
  const editor = await openedEditor(page);
  const localDraft =
    "# Browser editor fixture\n\nUnsaved because the daemon was downgraded.\n";

  await editor.fill(localDraft);

  await expect(page.getByRole("alert")).toContainText(
    "Page editing requires stable Wenlan daemon 0.14.1 or later. Running version: 0.14.0.",
    AUTOSAVE_TIMEOUT,
  );
  await expect.poll(() => editorSource(editor)).toBe(localDraft);
  expect(updateCalls(controller)).toHaveLength(1);
  expect(updateCalls(controller)[0]).toMatchObject({
    content: localDraft,
    expectedVersion: 7,
  });

  // A refused write stops the automatic loop instead of retrying it.
  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toHaveLength(1);
  expect(await editorSource(editor)).toBe(localDraft);

  expect(await storedPage(page)).toMatchObject({
    id: PAGE_ID,
    content: INITIAL_SOURCE,
    version: 7,
  });
});

test("falls back to the basic editor and autosaves exact source after module load failure", async ({ page }) => {
  await page.route(
    /\/(?:src\/components\/memory\/editor\/CodeMirrorMarkdownEditor\.tsx|assets\/CodeMirrorMarkdownEditor-[^/]+\.js)(?:\?|$)/,
    (route) => route.abort("failed"),
  );
  const controller = await openPage(page);

  const editor = await openedEditor(page);
  await expect(page.getByText("Basic editor active", { exact: true })).toBeVisible();
  await expect(editor).toHaveJSProperty("tagName", "TEXTAREA");
  await expect(page.getByRole("radio")).toHaveCount(0);
  await expect(page.getByRole("group", { name: "Formatting" })).toHaveCount(0);
  for (const button of persistenceButtons(page)) await expect(button).toHaveCount(0);
  await markEditorSession(editor);

  const fallbackSource =
    "# Browser editor fixture\n\nSaved exactly through the basic fallback.  \n";
  await editor.fill(fallbackSource);

  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(1);
  expect(updateCalls(controller)[0]).toMatchObject({
    id: PAGE_ID,
    content: fallbackSource,
    expectedVersion: 7,
    callerId: "wenlan-app",
  });
  await expect.poll(
    () => canonicalReadsAfterUpdate(controller, 1),
    AUTOSAVE_TIMEOUT,
  ).toBeGreaterThan(0);
  expect(await storedPage(page)).toMatchObject({ content: fallbackSource, version: 8 });
  await expect(editor).toHaveValue(fallbackSource);
  expect(await isSameEditorSession(editor)).toBe(true);
  await expect.poll(() =>
    controller.calls().filter(
      (call) => call.command === "record_page_editor_diagnostic",
    ).map((call) => call.args)
  ).toEqual([{ event: "editor_fallback", reason: "load" }]);
});

test("gives a long document workspace scrolling without a formatting toolbar or overflow", async ({ page }) => {
  const longSource = [
    "# Browser editor fixture",
    "",
    ...Array.from({ length: 180 }, (_, index) =>
      `${index + 1}. Long source line ${index + 1} stays readable and editable.`
    ),
  ].join("\n");
  await page.setViewportSize({ width: 1280, height: 900 });
  const controller = await openPage(page, pageFixture(longSource));
  // CodeMirror virtualizes lines; assert the document model, not 180 DOM rows.
  await openedEditor(page, longSource);

  const workspace = page.locator(".wiki-workspace-content");
  await expect.poll(() => workspace.evaluate((element) => element.scrollHeight - element.clientHeight)).toBeGreaterThan(0);
  await workspace.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect.poll(() => workspace.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);
  await workspace.evaluate((element) => {
    element.scrollTop = 0;
  });

  await expect(page.getByRole("radio")).toHaveCount(0);
  for (const button of persistenceButtons(page)) await expect(button).toHaveCount(0);

  const scroller = page.locator(
    '[data-markdown-editor-engine="codemirror"] .cm-scroller',
  );
  const geometry = await scroller.evaluate((element) => {
    element.scrollTop = 128;
    const style = window.getComputedStyle(element);
    return {
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
      overflowY: style.overflowY,
      scrollTop: element.scrollTop,
    };
  });
  expect(geometry.overflowY).toBe("visible");
  expect(geometry.scrollHeight).toBeLessThanOrEqual(geometry.clientHeight + 1);
  expect(geometry.scrollTop).toBe(0);

  await workspace.evaluate((element) => {
    element.scrollTop = 240;
  });
  await expect.poll(() => workspace.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);

  await expect(page.getByRole("toolbar", { name: "Formatting", exact: true })).toHaveCount(0);
  await expect(page.getByRole("group", { name: "Formatting", exact: true })).toHaveCount(0);
  expect(await workspace.evaluate((element) => element.scrollWidth - element.clientWidth))
    .toBeLessThanOrEqual(1);
  const actions = page.getByRole("button", { name: "Page actions", exact: true });
  await expect(actions).toBeEnabled();
  await actions.focus();
  await expect(actions).toBeFocused();

  // Scrolling and focusing a control are not edits.
  await page.waitForTimeout(QUIET_WINDOW_MS);
  expect(updateCalls(controller)).toEqual([]);
});

test("fits a 375px viewport without persistence actions, a formatting toolbar, or overflow", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await openPage(page);
  const editor = await openedEditor(page);

  for (const button of persistenceButtons(page)) await expect(button).toHaveCount(0);

  await expect(page.getByRole("toolbar", { name: "Formatting", exact: true })).toHaveCount(0);
  await expect(page.getByRole("group", { name: "Formatting", exact: true })).toHaveCount(0);
  const actions = page.getByRole("button", { name: "Page actions", exact: true });
  await expect(actions).toBeInViewport();

  expect(await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }))).toEqual({ clientWidth: 375, scrollWidth: 375 });

  const editorBox = await editor.boundingBox();
  expect(editorBox).not.toBeNull();
  expect(editorBox!.x).toBeGreaterThanOrEqual(0);
  expect(editorBox!.x + editorBox!.width).toBeLessThanOrEqual(375);
});

test("renders a borderless dark editor while page-actions focus stays visible", async ({ page }) => {
  await openPage(page);
  await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));

  const editor = await openedEditor(page);
  const editorFrame = page.locator('[data-markdown-editor-engine="codemirror"]');
  const editorSurface = editorFrame.locator(".cm-editor");
  await expect(editorSurface).toBeVisible();
  await expect(page.getByRole("toolbar", { name: "Formatting", exact: true })).toHaveCount(0);

  await editor.focus();
  await expect(editor).toBeFocused();
  await expect(editorSurface).toHaveClass(/cm-focused/);
  for (const surface of [editorFrame, editorSurface]) {
    expect(await surface.evaluate((element) => {
      const style = window.getComputedStyle(element);
      const sides = ["Top", "Right", "Bottom", "Left"] as const;
      return {
        visibleBorders: sides.filter((side) =>
          style.getPropertyValue(`border-${side.toLowerCase()}-style`) !== "none"
          && style.getPropertyValue(`border-${side.toLowerCase()}-width`) !== "0px"
        ),
        transparentBackground:
          style.backgroundColor === "transparent"
          || style.backgroundColor === "rgba(0, 0, 0, 0)",
      };
    })).toEqual({ visibleBorders: [], transparentBackground: true });
  }

  const actions = page.getByRole("button", { name: "Page actions", exact: true });
  await actions.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(actions).toBeFocused();
  expect(await actions.evaluate((element) => {
    const style = window.getComputedStyle(element);
    return {
      accent: window.getComputedStyle(document.documentElement)
        .getPropertyValue("--mem-accent-page")
        .trim().toLowerCase(),
      outlineColor: style.outlineColor,
      outlineVisible: style.outlineStyle !== "none" && style.outlineWidth !== "0px",
    };
  })).toEqual({
    accent: "#8fb3ea",
    outlineColor: "rgb(143, 179, 234)",
    outlineVisible: true,
  });
});

test("uses opaque light-theme focus indicators on page actions and tasks", async ({ page }) => {
  const source = "# Browser editor fixture\n\n- [ ] Keyboard task\n";
  await openPage(page, pageFixture(source));
  const editor = await openedEditor(page, source);
  const actions = page.getByRole("button", { name: "Page actions", exact: true });
  const task = editor.locator(".cm-writing-task-checkbox");

  await actions.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(actions).toBeFocused();
  expect(await actions.evaluate((element) => {
    const style = window.getComputedStyle(element);
    return {
      accent: window.getComputedStyle(document.documentElement)
        .getPropertyValue("--mem-accent-page")
        .trim().toLowerCase(),
      outlineColor: style.outlineColor,
    };
  })).toEqual({
    accent: "#5e58c8",
    outlineColor: "rgb(94, 88, 200)",
  });

  await task.focus();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  await expect(task).toBeFocused();
  expect(await task.evaluate((element) =>
    window.getComputedStyle(element).outlineColor
  )).toBe("rgb(94, 88, 200)");
});

test("gives a long document most of the available desktop height", async ({ page }) => {
  const longSource = Array.from(
    { length: 180 },
    (_, index) => `${index + 1}. Long source line ${index + 1}.`,
  ).join("\n");
  await page.setViewportSize({ width: 1280, height: 900 });
  await openPage(page, pageFixture(longSource));
  await openedEditor(page, longSource);

  const scroller = page.locator(
    '[data-markdown-editor-engine="codemirror"] .cm-scroller',
  );
  await expect.poll(() =>
    scroller.evaluate((element) => element.clientHeight)
  ).toBeGreaterThanOrEqual(600);
});


test("confirms a remotely deleted page before offering guarded local draft discard", async ({ page }, testInfo) => {
  const controller = await openPage(page);
  const editor = await openedEditor(page);
  await page.evaluate(async id => window.__TAURI_INTERNALS__!.invoke("delete_page", { id }), PAGE_ID);
  const draft = "A local draft that must survive remote deletion.";
  await editor.fill(draft);
  await expect.poll(() => updateCalls(controller).length, AUTOSAVE_TIMEOUT).toBe(1);
  await expect(page.getByRole("alert")).toContainText("This page no longer exists. Copy your draft before closing.");
  await expect.poll(() => editorSource(editor)).toBe(draft);
  await expect(page.getByRole("button", { name: "Discard draft", exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("editor-deleted-before-recovery.png"), fullPage: true });
  page.once("dialog", dialog => dialog.dismiss());
  await page.getByRole("button", { name: "Discard draft", exact: true }).click();
  await expect.poll(() => editorSource(editor)).toBe(draft);
  page.once("dialog", dialog => dialog.accept());
  await page.getByRole("button", { name: "Discard draft", exact: true }).click();
  await expect(page.locator(".page-detail")).toHaveCount(0);
  expect(updateCalls(controller)).toHaveLength(1);
});
