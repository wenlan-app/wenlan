// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { resources } from "../src/i18n/resources";
import type { AppLocale } from "../src/i18n/locales";
import type { TauriMockBasisScenario } from "./tauriMock/types";
import type { Page as KnowledgePage } from "../src/lib/tauri";

/**
 * Notes that rely on other notes ("basis"): the everyday scenarios of the
 * note-bases spec, driven through the real UI on the Tauri mock.
 *
 *  1. Mark a related note as a source, from the Related notes list.
 *  2. A source note changed: the notice, the diff, "Confirm no impact".
 *  3. "Update from sources" on such a note stages a preview card.
 *  4. Home shows one line for a source note that 20 notes rely on.
 *  5. A source that was archived is called out in plain words.
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
const BASIS_TITLE = "Pricing";

const edits = {
  "page-history": [
    { version: 2, edited_by: "agent_refresh", agent: "claude-code", at: 1_783_700_000, minor: false, delta_summary: "Raised the price" },
    { version: 3, edited_by: "manual_edit", at: 1_783_710_000, minor: false, delta_summary: "Added the annual plan" },
  ],
};

function fixture(extraPages: readonly KnowledgePage[] = []) {
  const base = createSpacesNavigationFixture();
  const template = base.pages[0];
  const pages = base.pages.map((note) => note.id === "page-history"
    ? { ...note, title: BASIS_TITLE, version: 3, content: "# Pricing\n\nThe plan costs 12 dollars a month, or 100 dollars a year." }
    : note);
  return { ...base, pages: [...pages, ...extraPages.map((note) => ({ ...template, ...note }))] };
}

const priorRow = (pageId: string, basisPageId = "page-history", basisTitle = BASIS_TITLE) => ({
  pageId,
  basisPageId,
  basisTitle,
  fromVersion: 1,
  fromContent: "# Pricing\n\nThe plan costs 10 dollars a month.",
});

type Setup = {
  readonly basis?: TauriMockBasisScenario;
  readonly pages?: readonly KnowledgePage[];
  readonly related?: boolean;
  readonly pendingRevisions?: boolean;
};

async function install(page: Page, locale: AppLocale, setup: Setup) {
  const base = fixture(setup.pages);
  const withReview = setup.pendingRevisions
    ? {
        ...base,
        pendingRevisions: [{
          target_source_id: "page-architecture",
          revision_source_id: "card-1",
          revision_content: "# Fixture architecture\n\nRebuilt from its sources and the Pricing note.",
          target_kind: "page" as const,
          source_agent: "wenlan-app",
          last_modified: 1_789_295_400,
        }],
      }
    : base;
  return installTauriMock(page, {
    locale,
    rawActions: [],
    fixture: withReview,
    pageScenario: {
      basis: setup.basis,
      outboundLinks: setup.related
        ? { "page-architecture": [{ label: BASIS_TITLE, target_page_id: "page-history" }] }
        : undefined,
    },
  });
}

async function openNote(page: Page, title = NOTE) {
  await page.goto("/");
  await openWikiNote(page, title);
  await expect(page.getByRole("heading", { level: 1, name: title })).toBeVisible();
}

function callsOf(controller: Awaited<ReturnType<typeof installTauriMock>>, command: string) {
  return controller.calls().filter((call) => call.command === command).map((call) => call.args);
}

for (const locale of LOCALES) {
  const t = copyFor(locale);

  test.describe(`note bases (${locale})`, () => {
    test("marks a related note as a source and explains the badge once", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, { related: true });
      await openNote(page);

      await page.getByRole("button", { name: t("pageInspector.open"), exact: true }).click();
      const related = page.getByRole("region", { name: t("knowledgeContext.linkedPages") });
      await expect(related).toBeVisible();
      await expect(related.getByText(t("noteBasis.badge"), { exact: true })).toHaveCount(0);

      await related.getByRole("button", { name: t("noteBasis.moreActions"), exact: true }).click();
      const item = page.getByRole("menuitem", { name: new RegExp(t("noteBasis.useAsSource")) });
      await expect(item).toContainText(t("noteBasis.useAsSourceHint"));
      await page.screenshot({ path: testInfo.outputPath(`mark-menu-${locale}.png`) });
      await item.click();

      const badge = related.getByText(t("noteBasis.badge"), { exact: true });
      await expect(badge).toBeVisible();
      expect(callsOf(controller, "mark_page_basis")).toEqual([{ pageId: "page-architecture", basisPageId: "page-history" }]);

      await badge.hover();
      await expect(page.getByRole("tooltip")).toHaveText(t("noteBasis.badgeTip"));
      // The tip keeps a readable width and stays inside the Related notes column.
      const tipBox = await page.getByRole("tooltip").boundingBox();
      const panelBox = await related.boundingBox();
      expect(tipBox).not.toBeNull();
      expect(panelBox).not.toBeNull();
      expect(tipBox!.width).toBeGreaterThan(120);
      expect(tipBox!.height).toBeLessThan(120);
      expect(tipBox!.x).toBeGreaterThanOrEqual(panelBox!.x);
      expect(tipBox!.x + tipBox!.width).toBeLessThanOrEqual(panelBox!.x + panelBox!.width);
      await page.screenshot({ path: testInfo.outputPath(`mark-badge-${locale}.png`) });
      await page.mouse.move(0, 0);
      await expect(page.getByRole("tooltip")).toHaveCount(0);
      await badge.hover();
      await expect(page.getByRole("tooltip")).toHaveCount(0);

      // A source that is up to date asks for nothing.
      await expect(page.getByText(t("noteBasis.pendingLine", { title: BASIS_TITLE }))).toHaveCount(0);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("links a note while typing [[ and uses it as a source in the same step", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {});
      await openNote(page);

      const editor = page.getByRole("textbox", { name: t("pageDetail.editor.label"), exact: true });
      await expect(editor).toBeEditable();
      await editor.click();
      await page.keyboard.press("ControlOrMeta+End");
      await page.keyboard.type(" [[pri");

      const menu = page.getByRole("listbox", { name: t("linkCompletion.label"), exact: true });
      await expect(menu).toBeVisible();
      const option = menu.getByRole("option", { name: BASIS_TITLE, exact: true });
      await expect(option).toHaveAttribute("aria-selected", "true");
      const action = option.getByRole("button", { name: t("noteBasis.useAsSource"), exact: true });
      await expect(action).toBeVisible();
      await expect(action).toHaveAttribute("title", t("noteBasis.useAsSourceHint"));
      await page.screenshot({ path: testInfo.outputPath(`link-menu-${locale}.png`) });
      await action.click();

      await expect(menu).toHaveCount(0);
      await expect(editor).toBeFocused();
      await expect(editor.getByRole("link", { name: BASIS_TITLE, exact: true })).toBeVisible();
      await expect.poll(() => callsOf(controller, "mark_page_basis"))
        .toEqual([{ pageId: "page-architecture", basisPageId: "page-history" }]);
      await expect.poll(() => callsOf(controller, "update_page").some((args) =>
        String((args as { content?: string }).content ?? "").includes(`[[${BASIS_TITLE}]]`))).toBe(true);
      await page.screenshot({ path: testInfo.outputPath(`link-picked-${locale}.png`) });
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("tells the person a source changed, shows the changes and confirms no impact", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        related: true,
        basis: { rows: [priorRow("page-architecture")], edits },
      });
      await openNote(page);

      const line = page.getByText(t("noteBasis.pendingLine", { title: BASIS_TITLE }), { exact: true });
      await expect(line).toBeVisible();
      await expect(page.getByRole("button", { name: t("noteBasis.confirmNoImpact"), exact: true })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath(`pending-notice-${locale}.png`) });

      await page.getByRole("button", { name: t("noteBasis.seeChanges"), exact: true }).click();
      const dialog = page.getByRole("dialog", { name: t("noteBasis.diff.title") });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByText(t("noteBasis.diff.versions", { title: BASIS_TITLE, from: 1, to: 3 }))).toBeVisible();
      await expect(dialog.locator("del").first()).toBeVisible();
      await expect(dialog.locator("ins").first()).toBeVisible();
      // The daemon's edit summaries can carry writer slugs and source ids; the
      // word diff above shows the change, so no language shows them.
      await expect(dialog.getByText("Raised the price")).toHaveCount(0);
      await expect(dialog.getByText("Added the annual plan")).toHaveCount(0);
      const editList = dialog.getByRole("region", { name: t("noteBasis.diff.edits") });
      await expect(editList.getByRole("listitem").nth(0)).toContainText(
        t("revisionLabel.writer.agentRefreshNamed", { agent: "Claude Code" }),
      );
      await expect(editList.getByRole("listitem").nth(1)).toContainText(t("noteBasis.diff.you"));
      await expect(editList).not.toContainText(/agent_refresh|manual_edit|human|claude-code/);
      await page.screenshot({ path: testInfo.outputPath(`pending-diff-${locale}.png`) });

      await dialog.getByRole("button", { name: t("noteBasis.confirmNoImpact"), exact: true }).click();
      await expect(dialog).toHaveCount(0);
      await expect(line).toHaveCount(0);
      expect(callsOf(controller, "confirm_page_basis")).toEqual([
        { pageId: "page-architecture", basisPageId: "page-history", version: 3 },
      ]);
      await page.screenshot({ path: testInfo.outputPath(`pending-confirmed-${locale}.png`) });

      // The confirmation stuck: the note now lists the source as up to date.
      const reread = await page.evaluate(async () =>
        window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { basis?: { state: string }[] };
      expect(reread.basis?.map((entry) => entry.state)).toEqual(["ok"]);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("updating from sources shows a preview to review instead of changing the note", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        pendingRevisions: true,
        basis: { rows: [priorRow("page-architecture")], edits, redistill: { "page-architecture": { gated: "card-1" } } },
      });
      await openNote(page);

      await page.getByRole("button", { name: t("pageDetail.actions"), exact: true }).click();
      await page.getByRole("menuitem", { name: t("pageDetail.redistillPage"), exact: true }).click();

      expect(callsOf(controller, "redistill_page")).toEqual([{ pageId: "page-architecture" }]);
      // The review queue opens on the preview card; nothing was applied to the note.
      const card = page.getByRole("dialog");
      await expect(card).toBeVisible();
      await expect(card.getByRole("heading", { name: NOTE, exact: true })).toBeVisible();
      await expect(card.locator("ins").first()).toBeVisible();
      // The app's own writer id never shows; the card says "Wenlan".
      await expect(card).toContainText(t("review.proposedBy", { agent: "Wenlan" }));
      await expect(card).not.toContainText("wenlan-app");
      await page.screenshot({ path: testInfo.outputPath(`update-preview-${locale}.png`) });
      const unchanged = await page.evaluate(async () =>
        window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { version: number };
      expect(unchanged.version).toBe(1);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("home shows one line for a source note that twenty notes rely on", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const plans = Array.from({ length: 20 }, (_, index) => ({
        id: `plan-${String(index + 1).padStart(2, "0")}`,
        title: `Plan ${String(index + 1).padStart(2, "0")}`,
      })) as unknown as KnowledgePage[];
      await install(page, locale, {
        pages: plans,
        basis: { rows: plans.map((plan) => priorRow(plan.id)), edits },
      });
      await page.goto("/");

      const summary = page.getByRole("list", { name: t("noteBasis.review.group") });
      await expect(summary.getByRole("listitem")).toHaveCount(1);
      await expect(summary).toContainText(t("noteBasis.review.summary_other", { title: BASIS_TITLE, count: 20 }));
      // A visible action, in a compact notice at the top of the pane.
      const line = summary.getByRole("button", { name: t("noteBasis.review.action"), exact: true });
      await expect(line).toBeVisible();
      await expect(line).toHaveCount(1);
      const noticeBox = await summary.boundingBox();
      const headingBox = await page.getByRole("heading", { level: 1 }).first().boundingBox();
      expect(noticeBox!.y).toBeLessThan(headingBox!.y);
      await page.screenshot({ path: testInfo.outputPath(`home-summary-${locale}.png`) });

      await line.click();
      const group = page.getByRole("region", { name: t("noteBasis.review.group") });
      await expect(group).toBeVisible();
      await group.getByRole("button", { name: t("noteBasis.review.expand"), exact: true }).click();
      await expect(group.getByRole("button", { name: t("noteBasis.confirmNoImpact"), exact: true })).toHaveCount(20);
      await page.screenshot({ path: testInfo.outputPath(`review-group-${locale}.png`) });
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("home says nothing about source updates when none are waiting", async ({ page }) => {
      const errors = collectBrowserErrors(page);
      await install(page, locale, { basis: { rows: [] } });
      await page.goto("/");

      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      await expect(page.getByRole("list", { name: t("noteBasis.review.group") })).toHaveCount(0);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });

    test("calls out a source that was archived and lets the person stop relying on it", async ({ page }, testInfo) => {
      const errors = collectBrowserErrors(page);
      const controller = await install(page, locale, {
        pages: [{ id: "page-retired", title: "Retired pricing", status: "archived" } as KnowledgePage],
        basis: { rows: [priorRow("page-architecture", "page-retired", "Retired pricing")] },
      });
      await openNote(page);

      const line = page.getByText(t("noteBasis.staleArchived", { title: "Retired pricing" }), { exact: true });
      await expect(line).toBeVisible();
      // Nothing to confirm for a source that is gone from view: only a way out.
      await expect(page.getByRole("button", { name: t("noteBasis.confirmNoImpact"), exact: true })).toHaveCount(0);
      await expect(page.getByRole("button", { name: t("noteBasis.seeChanges"), exact: true })).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath(`archived-${locale}.png`) });

      await page.getByRole("button", { name: t("noteBasis.stopUsing"), exact: true }).click();
      await expect(line).toHaveCount(0);
      expect(callsOf(controller, "unmark_page_basis")).toEqual([
        { pageId: "page-architecture", basisPageId: "page-retired" },
      ]);
      expect(errors.pageErrors).toEqual([]);
      expect(errors.consoleErrors).toEqual([]);
    });
  });
}
