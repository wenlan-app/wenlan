// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page as BrowserPage } from "@playwright/test";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

const evidenceDirectory = process.env.WENLAN_UI_EVIDENCE_DIR || "/tmp/wenlan-wiki-ui-evidence";

function wikiPages() {
  const defaults = createSpacesNavigationFixture().pages;
  const [topic, entity, decision, recap, ambient, person, policy] = defaults;
  if (!topic || !entity || !decision || !recap || !ambient || !person || !policy) throw new Error("Wiki fixture requires seven base pages");
  return [
    { ...topic, title: "Wenlan product principles", summary: "Guiding principles shaping the future of Wenlan.", space: "Wenlan" },
    { ...entity, title: "Nash Su", summary: "Research partner in distributed cognition.", entity_id: "entity-ada", creation_kind: "entity", space: "Research" },
    { ...decision, title: "Why citations stay visible", summary: "Keep citations visible to preserve trust and traceability.", content: "Decision: keep citations visible.", space: "Wenlan" },
    { ...recap, title: "July research recap", summary: "Key findings and open questions from July.", space: "Research" },
    { ...ambient, title: "Ambient memory", summary: "Capturing and surfacing useful context over time.", domain: null, space: null },
    { ...person, title: "Grace Hopper", summary: "Pioneer of programming languages and tooling.", entity_id: "entity-grace", creation_kind: "entity", domain: null, space: null },
    { ...policy, title: "Source credibility policy", summary: "Ensure sources are evaluated for credibility and trust.", content: "Decision: evaluate every source.", space: "Wenlan" },
    { ...topic, id: "page-eighth", title: "Memory provenance", summary: "Where durable context comes from.", last_modified: "2026-07-05T12:00:00Z", space: "Research", storage_path: "Research/memory-provenance.md" },
    { ...topic, id: "page-ninth", title: "Local-first architecture", summary: "Private knowledge stays on this device.", last_modified: "2026-07-04T12:00:00Z", space: "Wenlan", storage_path: "Research/Notes/local-first-architecture.md" },
  ];
}

async function openWiki(page: BrowserPage, locale: "en" | "zh-Hant", theme: "dark" | "light", lens: "rows" | "cards") {
  const browserErrors = collectBrowserErrors(page);
  await installTauriMock(page, {
    fixture: {
      ...createSpacesNavigationFixture(),
      folders: [
        { path: "Research", parent_path: "", name: "Research" },
        { path: "Research/Notes", parent_path: "Research", name: "Pages" },
      ],
      pages: wikiPages(),
    },
    locale,
    localStorage: { "wenlan-theme": theme, "wenlan-wiki-view-mode": lens },
    preserveLocalStorage: true,
    rawActions: [],
  });
  await page.goto("/");

  if ((page.viewportSize()?.width ?? 0) < 900) {
    await page.getByTitle(locale === "zh-Hant" ? "顯示側邊欄" : "Show sidebar").click();
  }
  const primaryNavigation = page.getByRole("navigation", { name: locale === "zh-Hant" ? "主要導覽" : "Primary navigation" });
  await primaryNavigation.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: locale === "zh-Hant" ? "開啟筆記" : "Open a page" })).toBeVisible();
  if ((page.viewportSize()?.width ?? 0) < 900) await page.waitForTimeout(250);
  return browserErrors;
}


for(const locale of ["en","zh-Hant"] as const)for(const theme of ["light","dark"] as const)for(const width of [1280,375])test(`Wiki reading workspace ${locale} ${theme} ${width}`,async({page},info)=>{
 await page.setViewportSize({width,height:900});const errors=await openWiki(page,locale,theme,"cards");
 const right=page.locator(".wiki-workspace-reading"),tree=page.locator(".wiki-workspace-directory");
 await expect(right.locator(".wiki-table, .asset-card, .wiki-folder-lenses")).toHaveCount(0);
 await expect(page.locator(".note-tab-create")).toBeVisible();
 if(!(await tree.isVisible()))await page.locator(".wiki-workspace-folders-toggle").click();
 const inventoryMode=tree.getByRole("combobox");
 await inventoryMode.selectOption("folders");
 await expect(inventoryMode).toHaveValue("folders");
 await tree.getByRole("button",{name:"Research",exact:true}).click();
 await expect(tree.getByTitle("Memory provenance",{exact:true})).toBeVisible();
 await expect(tree.getByTitle("Local-first architecture",{exact:true})).toHaveCount(0);
 await expect(right).not.toContainText("Memory provenance");
 await tree.getByRole("button",{name:"Pages",exact:true}).click();
 await expect(tree.getByTitle("Local-first architecture",{exact:true})).toBeVisible();
 await expect(tree.getByTitle("Nash Su",{exact:true})).toHaveCount(0);
 await expect(tree.getByTitle("Grace Hopper",{exact:true})).toHaveCount(0);
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
 await page.screenshot({path:info.outputPath("reading-start.png")});
 await openWikiNote(page,"Local-first architecture");
 await expect(page.getByRole("heading",{level:1,name:"Local-first architecture",exact:true})).toBeVisible();
 await expect(page.getByRole("tab",{name:"Local-first architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await page.screenshot({path:info.outputPath("reading-note.png")});
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

test("Cmd+K event opens the responsive global search instead of focusing a hidden input", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 812 });
  const browserErrors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "zh-Hant", rawActions: [] });
  await page.goto("/");

  await page.evaluate(async () => {
    await window.__TAURI_INTERNALS__?.invoke("plugin:event|emit", { event: "toggle-spotlight", payload: null });
  });

  const searchInput = page.getByPlaceholder("搜尋筆記、記憶、來源...");
  await expect(searchInput).toBeVisible();
  await expect(searchInput).toBeFocused();
  await expect(page.getByRole("dialog", { name: "搜尋", exact: true })).toHaveAttribute("aria-modal", "true");
  await expect(page.locator(".memory-workspace-header input")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "搜尋", exact: true })).toHaveAttribute("aria-expanded", "true");
  await page.screenshot({ path: `${evidenceDirectory}/wiki-zh-hant-shortcut-search-375x812.png`, fullPage: true });
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
