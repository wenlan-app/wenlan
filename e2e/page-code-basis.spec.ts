// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { resources } from "../src/i18n/resources";
import type { AppLocale } from "../src/i18n/locales";
import type { MockCodeBasisRow, TauriMockCodeBasisScenario } from "./tauriMock/pageCodeBasis";
import type { Page as KnowledgePage } from "../src/lib/tauri";

/**
 * Notes that rely on lines of code ("code basis"): the scenarios of the
 * approved design, driven through the real UI on the Tauri mock.
 *
 *  1. A note whose code changed: the notice, "Confirm no impact", and a
 *     second look when the code changed again.
 *  2. Code that cannot be found: called out in plain words, only a way out.
 *  3. "See code": the lines as cited next to the lines now.
 *  4. The Info rail lists the code and copies a location.
 *  5. Home shows one line, and the review has a group for it.
 */

const LOCALES: readonly AppLocale[] = ["en", "zh-Hant", "zh-Hans"];

/** Copy comes from the shipped resources, so a rename cannot slip past this spec. */
function copyFor(locale: AppLocale) {
  const root = resources[locale].translation as unknown as Record<string, unknown>;
  return (key: string, vars: Record<string, string | number> = {}): string => {
    const lookup = (name: string): unknown =>
      name.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], root);
    let value = lookup(key);
    if (typeof vars.count === "number") {
      const plural = locale === "en" && vars.count === 1 ? "_one" : "_other";
      value = lookup(`${key}${plural}`) ?? value;
    }
    if (typeof value !== "string") throw new Error(`missing copy: ${key} (${locale})`);
    return value.replace(/\{\{(\w+)\}\}/g, (_match, name: string) => String(vars[name] ?? ""));
  };
}

const NOTE = "Fixture architecture";
const REPO = "/work/wenlan";
const FILE = "src/parser.rs";

const CITED_BLOB = "0123456789abcdef0123456789abcdef01234567";
// The cited lines are 10 to 13 of the file: nine lines come before them.
const HEAD = Array.from({ length: 9 }, (_, index) => `// line ${index + 1}`).join("\n");
const CITED = `${HEAD}\nfn parse(input: &str) {\n    let tokens = lex(input);\n    build(tokens)\n}\n`;
const NOW = `${HEAD}\nfn parse(input: &str) {\n    let tokens = lex_v2(input);\n    build(tokens)\n}\n`;

const row = (pageId: string, over: Partial<MockCodeBasisRow> = {}): MockCodeBasisRow => ({
  pageId,
  repoRoot: REPO,
  path: FILE,
  lineStart: 10,
  lineEnd: 13,
  state: "pending",
  changedAt: 1_783_700_000,
  citedBlob: CITED_BLOB,
  ...over,
});

const locatorOf = (over: Record<string, unknown> = {}) => ({
  repo_root: REPO,
  path: FILE,
  line_start: 10,
  line_end: 13,
  ...over,
});

type Setup = {
  readonly codeBasis?: TauriMockCodeBasisScenario;
  readonly pages?: readonly KnowledgePage[];
};

async function install(page: Page, locale: AppLocale, setup: Setup) {
  const base = createSpacesNavigationFixture();
  const template = base.pages[0];
  const fixture = { ...base, pages: [...base.pages, ...(setup.pages ?? []).map((note) => ({ ...template, ...note }))] };
  return installTauriMock(page, {
    locale,
    rawActions: [],
    fixture,
    pageScenario: { codeBasis: setup.codeBasis },
  });
}

const withFile = (rows: readonly MockCodeBasisRow[]): TauriMockCodeBasisScenario => ({
  rows,
  files: { [`${REPO}/${FILE}`]: NOW },
  blobs: { [CITED_BLOB]: CITED },
});

async function openNote(page: Page, title = NOTE) {
  await page.goto("/");
  await openWikiNote(page, title);
  await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
}

function callsOf(controller: Awaited<ReturnType<typeof installTauriMock>>, command: string) {
  return controller.calls().filter((call) => call.command === command).map((call) => call.args);
}

/** The text the app last handed to the clipboard (the mock answers plugin calls with nothing). */
function lastCopied(controller: Awaited<ReturnType<typeof installTauriMock>>): string {
  const writes = controller.calls().filter((call) => call.command.startsWith("plugin:") && call.command.includes("clipboard"));
  return JSON.stringify(writes.at(-1)?.args ?? null);
}

for (const locale of LOCALES) {
  const t = copyFor(locale);
  const span = { file: FILE, from: 10, to: 13 };

  test.describe(`code basis (${locale})`, () => {
    test("tells the person the code changed and confirms no impact", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, { codeBasis: withFile([row("page-architecture")]) });
      await openNote(page);

      const line = page.getByText(t("codeBasis.changedLine", span), { exact: true });
      await expect(line).toBeVisible();
      await expect(page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath(`code-notice-${locale}.png`) });

      await page.getByRole("button", { name: t("codeBasis.confirmNoImpact"), exact: true }).click();
      await expect(line).toHaveCount(0);
      expect(callsOf(controller, "confirm_page_code_basis")).toEqual([
        { pageId: "page-architecture", locator: locatorOf() },
      ]);

      // The confirmation stuck: the note now lists the lines as up to date.
      const reread = await page.evaluate(async () =>
        window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { code_basis?: { state: string }[] };
      expect(reread.code_basis?.map((entry) => entry.state)).toEqual(["ok"]);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("asks for another look when the code changed again, then confirms", async ({ page }) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        codeBasis: withFile([row("page-architecture", { changesAgain: true })]),
      });
      await openNote(page);

      const confirm = page.getByRole("button", { name: t("codeBasis.confirmNoImpact"), exact: true });
      await confirm.click();
      await expect(page.getByText(t("codeBasis.changedAgain"), { exact: true })).toBeVisible();
      await expect(page.getByText(t("codeBasis.changedLine", span), { exact: true })).toBeVisible();

      await confirm.click();
      await expect(page.getByText(t("codeBasis.changedLine", span), { exact: true })).toHaveCount(0);
      expect(callsOf(controller, "confirm_page_code_basis")).toHaveLength(2);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("calls out code that cannot be found and lets the person stop relying on it", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        codeBasis: {
          rows: [row("page-architecture", { path: "src/gone.rs", state: "unavailable", reason: "file_missing", lineStart: 4, lineEnd: 9 })],
        },
      });
      await openNote(page);

      const line = page.getByText(t("codeBasis.missingFile", { file: "src/gone.rs" }), { exact: true });
      await expect(line).toBeVisible();
      // Nothing to confirm for code that is gone: only a way out.
      await expect(page.getByRole("button", { name: t("codeBasis.confirmNoImpact"), exact: true })).toHaveCount(0);
      await expect(page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true })).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath(`code-missing-${locale}.png`) });

      await page.getByRole("button", { name: t("codeBasis.stopRelying"), exact: true }).click();
      await expect(line).toHaveCount(0);
      expect(callsOf(controller, "remove_page_code_basis")).toEqual([
        { pageId: "page-architecture", locator: locatorOf({ path: "src/gone.rs", line_start: 4, line_end: 9 }) },
      ]);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("shows the lines as cited next to the lines now", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, { codeBasis: withFile([row("page-architecture")]) });
      await openNote(page);

      await page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true }).click();
      const dialog = page.getByRole("dialog", { name: t("codeBasis.dialog.title", span) });
      await expect(dialog).toBeVisible();
      const before = dialog.getByTestId("code-basis-before");
      const after = dialog.getByTestId("code-basis-after");
      await expect(dialog.getByRole("heading", { name: t("codeBasis.dialog.before"), exact: true })).toBeVisible();
      await expect(dialog.getByRole("heading", { name: t("codeBasis.dialog.after"), exact: true })).toBeVisible();
      await expect(before).toContainText("lex(input)");
      await expect(before).not.toContainText("lex_v2");
      await expect(after).toContainText("lex_v2(input)");
      await expect(after.locator(".code-basis__number")).toHaveText(["10", "11", "12", "13"]);
      // Side by side, with the one changed line tinted on each side and the equal lines plain.
      const beforeBox = await before.boundingBox();
      const afterBox = await after.boundingBox();
      expect(afterBox!.x).toBeGreaterThan(beforeBox!.x + beforeBox!.width - 1);
      await expect(before.locator(".code-basis__line--removed")).toHaveCount(1);
      await expect(before.locator(".code-basis__line--removed")).toContainText("lex(input)");
      await expect(after.locator(".code-basis__line--added")).toHaveCount(1);
      await expect(after.locator(".code-basis__line--added")).toContainText("lex_v2(input)");
      await expect(dialog.locator(".code-basis__line")).toHaveCount(8);
      // The title is set in the UI face, not the serif heading face.
      expect(await dialog.getByRole("heading", { level: 2 }).evaluate((el) => getComputedStyle(el).fontFamily)).toContain("Instrument Sans");
      expect(callsOf(controller, "get_page_code_basis_cited")).toEqual([
        { pageId: "page-architecture", locator: locatorOf() },
      ]);
      expect(callsOf(controller, "read_git_blob_lines")).toEqual([
        { repoRoot: REPO, blob: CITED_BLOB, lineStart: 10, lineEnd: 13 },
      ]);
      expect(callsOf(controller, "read_code_lines")).toEqual([
        { path: `${REPO}/${FILE}`, lineStart: 10, lineEnd: 13 },
      ]);
      await page.screenshot({ path: testInfo.outputPath(`code-dialog-${locale}.png`) });

      await dialog.getByRole("button", { name: t("codeBasis.copyLocation"), exact: true }).click();
      await expect(dialog.getByRole("button", { name: t("codeBasis.copied"), exact: true })).toBeVisible();
      expect(lastCopied(controller)).toContain(`${FILE}:10`);

      await dialog.getByRole("button", { name: t("codeBasis.close"), exact: true }).click();
      await expect(dialog).toHaveCount(0);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("keeps the tint to the far edge when scrolled, the box as tall as its lines, and focus out of the layout", async ({ page }) => {
      const errors = collectBrowserErrors(page);
      const long = `    let value = some_function_with_a_very_long_name(${Array.from({ length: 12 }, (_, i) => `argument_${i}`).join(", ")});`;
      const lines = (call: string, tail = "") => `${HEAD}\nfn parse() {\n    ${call}();\n    same();\n${long}${tail}\n}\n`;
      await install(page, locale, {
        codeBasis: {
          rows: [row("page-architecture", { lineEnd: 14 })],
          files: { [`${REPO}/${FILE}`]: lines("new_call", " // added") },
          blobs: { [CITED_BLOB]: lines("old_call") },
        },
      });
      await page.setViewportSize({ width: 1280, height: 900 });
      await openNote(page);
      await page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true }).click();
      const dialog = page.getByRole("dialog");
      for (const id of ["code-basis-before", "code-basis-after"]) {
        const box = dialog.getByTestId(id);
        await expect(box.locator(".code-basis__line")).toHaveCount(5);
        // Every row, tinted or not, is as wide as the longest line, not as wide as the box.
        const widths = await box.locator(".code-basis__line").evaluateAll((rows) => rows.map((row) => Math.round(row.getBoundingClientRect().width)));
        const scroller = box.locator(".code-basis__scroll");
        const scrollWidth = await scroller.evaluate((el) => el.scrollWidth);
        const clientWidth = await scroller.evaluate((el) => el.clientWidth);
        expect(scrollWidth).toBeGreaterThan(clientWidth);
        expect(new Set(widths)).toEqual(new Set([scrollWidth]));
        // Scrolled sideways, the tint still reaches the box's right edge.
        await scroller.evaluate((el) => { el.scrollLeft = el.scrollWidth; });
        const tinted = box.locator("[data-change]").first();
        const rowRight = await tinted.evaluate((el) => el.getBoundingClientRect().right);
        const boxRight = await scroller.evaluate((el) => el.getBoundingClientRect().right);
        expect(rowRight).toBeGreaterThanOrEqual(boxRight - 1);
        // Five lines fit: the box does not scroll up and down.
        expect(await box.evaluate((el) => el.scrollHeight <= el.clientHeight)).toBe(true);
        // One focusable scroller per box.
        await expect(box.locator("[tabindex]")).toHaveCount(1);
      }
      // Focusing a box changes neither its size nor the dialog's position.
      const after = dialog.getByTestId("code-basis-after");
      const measure = async () => ({ panel: await dialog.boundingBox(), box: await after.boundingBox(), top: await dialog.evaluate((el) => el.scrollTop) });
      const resting = await measure();
      await after.locator(".code-basis__text").nth(1).click();
      await page.keyboard.press("Tab");
      await page.keyboard.press("Shift+Tab");
      await expect(after.locator(".code-basis__scroll")).toBeFocused();
      expect(await measure()).toEqual(resting);
      expect(errors.pageErrors).toEqual([]);
    });

    test("sizes the one column to its lines when there is nothing to compare", async ({ page }) => {
      await install(page, locale, {
        codeBasis: { rows: [row("page-architecture", { citedBlob: null })], files: { [`${REPO}/${FILE}`]: NOW } },
      });
      await openNote(page);
      await page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true }).click();
      const box = page.getByRole("dialog").getByTestId("code-basis-after");
      await expect(box.locator(".code-basis__line")).toHaveCount(4);
      const { box: boxHeight, lines } = await box.evaluate((el) => ({
        box: el.getBoundingClientRect().height,
        lines: el.querySelector(".code-basis__lines")!.getBoundingClientRect().height,
      }));
      // The frame and padding around the lines, and nothing more.
      expect(boxHeight - lines).toBeLessThan(30);
    });

    test("says there is nothing to compare when the lines were never committed, and the file cannot be read when that is so", async ({ page }) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        // No file in the fixture: reading it fails, as it does for a file that is gone.
        codeBasis: { rows: [row("page-architecture", { citedBlob: null })] },
      });
      await openNote(page);

      await page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true }).click();
      const dialog = page.getByRole("dialog", { name: t("codeBasis.dialog.title", span) });
      await expect(dialog.getByText(t("codeBasis.dialog.noBefore"), { exact: true })).toBeVisible();
      await expect(dialog.getByText(t("codeBasis.dialog.unavailable"), { exact: true })).toBeVisible();
      // No blob id, so git is never asked.
      expect(callsOf(controller, "read_git_blob_lines")).toEqual([]);
      expect(errors.pageErrors).toEqual([]);
    });

    test("says the same when git has lost the cited version", async ({ page }) => {
      const errors = collectBrowserErrors(page);
      await install(page, locale, {
        codeBasis: { rows: [row("page-architecture")], files: { [`${REPO}/${FILE}`]: NOW } },
      });
      await openNote(page);

      await page.getByRole("button", { name: t("codeBasis.seeCode"), exact: true }).click();
      const dialog = page.getByRole("dialog", { name: t("codeBasis.dialog.title", span) });
      await expect(dialog.getByText(t("codeBasis.dialog.noBefore"), { exact: true })).toBeVisible();
      await expect(dialog.getByTestId("code-basis-after")).toContainText("lex_v2(input)");
      expect(errors.pageErrors).toEqual([]);
    });

    test("lists the code in the Info rail and copies a location", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        codeBasis: withFile([
          row("page-architecture"),
          row("page-architecture", { path: "src/lex.rs", lineStart: 3, lineEnd: 3, state: "ok", citedBlob: null }),
        ]),
      });
      await openNote(page);

      await page.getByRole("button", { name: t("pageInspector.open"), exact: true }).click();
      const group = page.getByRole("region", { name: t("codeBasis.groupLabel") });
      await expect(group).toBeVisible();
      await expect(group.getByRole("heading", { name: t("codeBasis.group"), exact: true })).toBeVisible();
      await expect(group.getByRole("listitem")).toHaveCount(2);
      await expect(group.getByText(`${FILE}:10\u201313`, { exact: true })).toBeVisible();
      // The range is set in the UI face, so its en dash does not look like a hyphen; the path stays mono.
      const range = group.locator(".note-info-code-range").first();
      await expect(range).toHaveText(":10\u201313");
      expect(await range.evaluate((el) => getComputedStyle(el).fontFamily)).toContain("Instrument Sans");
      expect(await group.locator(".note-info-code-path").first().evaluate((el) => getComputedStyle(el).fontFamily)).toContain("JetBrains Mono");
      await expect(group.getByText("src/lex.rs:3", { exact: true })).toBeVisible();
      await expect(group.getByText(t("codeBasis.state.pending"), { exact: true })).toBeVisible();
      await expect(group.getByText(t("codeBasis.state.ok"), { exact: true })).toBeVisible();
      // Only shown and removed here: there is no way to add code from the app.
      await expect(group.getByRole("button", { name: /add|新增|添加|加入/i })).toHaveCount(0);
      // The count joins the other parts of what the note is built on, in the Info summary.
      await expect(page.getByText(t("codeBasis.codeCount", { count: 2 })).first()).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath(`code-info-${locale}.png`) });

      await group.getByRole("button", { name: `${FILE}:10\u201313` }).click();
      await expect(group.getByText(t("codeBasis.copied"), { exact: true })).toBeVisible();
      expect(lastCopied(controller)).toContain(`${FILE}:10`);

      await group.getByRole("button", { name: t("codeBasis.moreActions"), exact: true }).first().click();
      await page.getByRole("menuitem", { name: t("codeBasis.stopRelying"), exact: true }).click();
      await expect(group.getByText(`${FILE}:10\u201313`, { exact: true })).toHaveCount(0);
      expect(callsOf(controller, "remove_page_code_basis")).toHaveLength(1);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("home shows one line for the notes whose code changed, and the review has a group for them", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const notes = [
        { id: "plan-01", title: "Plan 01" },
        { id: "plan-02", title: "Plan 02" },
      ] as unknown as KnowledgePage[];
      const controller = await install(page, locale, {
        pages: notes,
        codeBasis: withFile([
          row("plan-01"),
          row("plan-01", { path: "src/gone.rs", state: "unavailable", reason: "repo_missing", lineStart: 1, lineEnd: 2 }),
          row("plan-02"),
        ]),
      });
      await page.goto("/");

      const summary = page.getByRole("list", { name: t("codeBasis.review.group") });
      await expect(summary.getByRole("listitem")).toHaveCount(1);
      await expect(summary).toContainText(t("codeBasis.review.summary_other", { count: 2 }));
      const open = summary.getByRole("button", { name: t("codeBasis.review.action"), exact: true });
      await expect(open).toBeVisible();
      const noticeBox = await summary.boundingBox();
      const headingBox = await page.getByRole("heading", { level: 1 }).first().boundingBox();
      expect(noticeBox!.y).toBeLessThan(headingBox!.y);
      await page.screenshot({ path: testInfo.outputPath(`code-home-${locale}.png`) });

      await open.click();
      const group = page.getByRole("region", { name: t("codeBasis.review.group") });
      await expect(group).toBeVisible();
      await expect(group.getByRole("button", { name: "Plan 01", exact: true })).toBeVisible();
      await group.getByRole("button", { name: "Plan 01", exact: true }).click();
      await expect(group.getByText(t("codeBasis.changedLine", span), { exact: true })).toBeVisible();
      await expect(group.getByText(t("codeBasis.missingRepo", { file: "src/gone.rs" }), { exact: true })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath(`code-review-${locale}.png`) });

      await group.getByRole("button", { name: t("codeBasis.confirmNoImpact"), exact: true }).click();
      await expect(group.getByText(t("codeBasis.changedLine", span), { exact: true })).toHaveCount(0);
      expect(callsOf(controller, "confirm_page_code_basis")).toEqual([
        { pageId: "plan-01", locator: locatorOf() },
      ]);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("home says nothing about code when none changed", async ({ page }) => {
      const errors = collectBrowserErrors(page);
      await install(page, locale, { codeBasis: withFile([row("page-architecture", { state: "ok" })]) });
      await page.goto("/");

      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      await expect(page.getByRole("list", { name: t("codeBasis.review.group") })).toHaveCount(0);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });
  });
}
