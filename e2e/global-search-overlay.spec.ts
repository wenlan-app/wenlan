// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect, type Page, type Locator } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { openWikiNote } from "./helpers/wikiWorkspace";

const dialog = (page: Page) => page.getByRole("dialog", { name: "Search", exact: true });
const input = (page: Page) => dialog(page).getByPlaceholder("Search pages, memories, sources...");
async function emitSearch(page: Page) {
  await page.evaluate(async () => { await window.__TAURI_INTERNALS__?.invoke("plugin:event|emit", { event: "toggle-spotlight", payload: null }); });
  await expect(input(page)).toBeFocused();
}
async function start(page: Page, width = 1280, sidebar?: { visible: boolean; mode: string }) {
  await page.setViewportSize({ width, height: 900 });
  const errors = collectBrowserErrors(page);
  const fixture = createReviewDecisionFixture("wiki-folders");
  const runtime = await installTauriMock(page, { locale: "en", rawActions: [], fixture,
    delays: { update_page: 150 },
    localStorage: sidebar ? { "wenlan-navigation-v1": JSON.stringify({version: 1, visible: ["pages", "spaces", "graph", "sources"], sidebar}) } : {},
  });
  await page.addInitScript(({ target }) => {
    const api = window.__TAURI_INTERNALS__; if (!api) return;
    const original = api.invoke.bind(api);
    api.invoke = async (command, args, options) => {
      const q = typeof args === "object" && args !== null ? String(Reflect.get(args, "query") ?? "") : "";
      if (["search", "search_pages", "search_entities_cmd"].includes(command)) {
        if (q === "slow") await new Promise(resolve => setTimeout(resolve, 900));
        if (q === "error") throw new Error("Search unavailable for fixture");
        if (command === "search_pages") return q === "fixture" || q === "slow" ? [target] : [];
        if (command === "search") return q === "fixture" ? [
          {id:"memory-result",source_id:"memory-0",source:"memory",title:"A fixture memory",content:"A fixture memory to remember",url:null,chunk_index:0,last_modified:1789295400,score:0.9},
          {id:"source-result",source_id:"source-one",source:"file",title:"Fixture source",content:"A fixture source document",url:"file:///fixture/source.md",chunk_index:0,last_modified:1789295400,score:0.8},
        ] : [];
        return [];
      }
      return original(command, args, options);
    };
  }, { target: fixture.pages.find(p => p.title === "Why fixtures stay deterministic")! });
  await page.goto("/"); await expect(page.locator(".wiki-workspace")).toBeVisible();
  return { runtime, errors };
}
async function source(editor: Locator) { return editor.evaluate(el => (el as any).cmTile.root.view.state.doc.toString() as string); }

test("sidebar search opens a modal without a header textbox and restores focus", async ({ page }) => {
  const { runtime, errors } = await start(page);
  await expect(page.locator(".memory-workspace-header input")).toHaveCount(0);
  const trigger = page.locator(".notes-workspace-sidebar").getByRole("button", { name: "Search", exact: true });
  await expect(trigger).toBeVisible(); await trigger.click();
  await expect(dialog(page)).toHaveAttribute("aria-modal", "true"); await expect(input(page)).toBeFocused();
  await expect(page.locator(".memory-shell")).toHaveAttribute("inert", "");
  expect(runtime.calls().filter(c => ["search", "search_pages", "search_entities_cmd"].includes(c.command))).toHaveLength(0);
  await input(page).press("Shift+Tab");
  expect(await dialog(page).evaluate(el => el.contains(document.activeElement))).toBe(true);
  await input(page).fill("unused"); await dialog(page).getByRole("button", {name: "Close search", exact: true}).focus();
  await emitSearch(page); expect(await input(page).evaluate(el => (el as HTMLInputElement).selectionEnd! - (el as HTMLInputElement).selectionStart!)).toBe(6);
  await page.keyboard.press("Escape"); await expect(dialog(page)).toHaveCount(0); await expect(trigger).toBeFocused();
  await trigger.click(); await page.mouse.click(5, 5); await expect(dialog(page)).toHaveCount(0); await expect(trigger).toBeFocused();
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("search is reachable from hidden, icon and narrow sidebars", async ({ page }) => {
  await start(page,1280,{visible:false,mode:"labels"});
  const headerTrigger=page.locator(".note-group-global-tools").getByRole("button",{name:"Search",exact:true});
  await headerTrigger.click();await expect(input(page)).toBeFocused();await page.keyboard.press("Escape");await expect(headerTrigger).toBeFocused();
  await page.getByRole("button",{name:"Show sidebar",exact:true}).click();
  await page.setViewportSize({width:375,height:812});
  await headerTrigger.click();await expect(input(page)).toBeFocused();await page.keyboard.press("Escape");
  await page.getByRole("button",{name:"Show sidebar",exact:true}).click();
  await page.locator(".notes-workspace-sidebar").getByRole("button",{name:"Search",exact:true}).click();
  await expect(input(page)).toBeFocused();await expect(page.locator('[data-sidebar-overlay="true"]')).toHaveCount(0);
  await page.keyboard.press("Escape");await expect(dialog(page)).toHaveCount(0);
  expect(await page.evaluate(()=>document.activeElement?.getBoundingClientRect().width)).toBeGreaterThan(0);
});

test("icon sidebar keeps an accessible Search entry",async({page})=>{
  await start(page,1280,{visible:true,mode:"icons"});
  await page.locator(".notes-workspace-sidebar").getByRole("button",{name:"Search",exact:true}).click();await expect(input(page)).toBeFocused();
});

test("search keeps the same live editor, then opens the selected page through its guarded route", async ({ page }) => {
  const {runtime,errors}=await start(page);await openWikiNote(page,"Fixture architecture");
  const editor=page.getByRole("textbox",{name:"Page editor",exact:true});
  await editor.evaluate(el=>{el.setAttribute("data-search-session","same");const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});
  await page.keyboard.insertText("\nText retained through search.");await emitSearch(page);await input(page).fill("fixture");
  await expect(dialog(page).getByRole("button",{name:/Why fixtures stay deterministic/})).toBeVisible();
  await expect(page.locator('[data-search-session="same"]')).toHaveCount(1);expect(await source(page.locator('[data-search-session="same"]'))).toContain("Text retained through search.");
  await page.keyboard.press("Escape");await expect(dialog(page)).toHaveCount(0);await expect(editor).toHaveAttribute("data-search-session","same");
  await emitSearch(page);await input(page).fill("fixture");
  const target=dialog(page).getByRole("button",{name:/Why fixtures stay deterministic/});await target.focus();await target.press("Enter");
  await expect(dialog(page)).toHaveCount(0);await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
  expect(runtime.calls().filter(c=>c.command==="update_page")).toHaveLength(1);
  expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

test("failed flush keeps unsaved content and cancellation returns to that editor", async ({ page }) => {
  const {runtime}=await start(page);await openWikiNote(page,"Fixture architecture");
  const editor=page.getByRole("textbox",{name:"Page editor",exact:true});
  await editor.evaluate(el=>{el.setAttribute("data-search-session","failed");const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});
  runtime.failNext("update_page","save refused");await page.keyboard.insertText("\nKeep unsaved search edit.");await emitSearch(page);await input(page).fill("fixture");
  await expect(page.locator(".page-detail [role=alert]")).toBeAttached();
  await expect(page.locator('[data-search-session="failed"]')).toHaveCount(1);
  await page.keyboard.press("Escape");await expect(dialog(page)).toHaveCount(0);
  await expect(editor).toHaveAttribute("data-search-session","failed");expect(await source(editor)).toContain("Keep unsaved search edit.");
  await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
  expect(runtime.calls().filter(c=>c.command==="update_page")).toHaveLength(1);
});

test("modal shows loading and failure states plus typed groups and source navigation",async({page})=>{
 const {runtime}=await start(page);await emitSearch(page);await input(page).fill("slow");
 await expect(dialog(page).getByRole("status")).toContainText(/search|load/i);
 await expect(dialog(page).getByRole("button",{name:/Why fixtures stay deterministic/})).toBeVisible();
 await input(page).fill("error");await expect(dialog(page).getByRole("alert")).toBeVisible({timeout:10000});
 await input(page).fill("fixture");await expect(dialog(page).getByRole("button",{name:/A fixture memory/})).toBeVisible();
 await expect(dialog(page).getByRole("button",{name:/A fixture source/})).toBeVisible();
 await expect(dialog(page).locator("mark").first()).toContainText(/fixture/i);
 await input(page).press("ArrowDown");await expect(dialog(page).getByRole("button",{name:/Why fixtures stay deterministic/})).toBeFocused();
 await page.keyboard.press("ArrowUp");await expect(input(page)).toBeFocused();
 const sourceResult=dialog(page).getByRole("button",{name:/A fixture source/});await sourceResult.focus();await sourceResult.press("Enter");
 await expect.poll(()=>runtime.calls().filter(c=>c.command==="open_search_result").length).toBe(1);
 expect(runtime.calls().find(c=>c.command==="open_search_result")?.args).toMatchObject({url:"file:///fixture/source.md"});
});


test("Settings keeps keyboard search and slash stays inside the page editor",async({page})=>{
 await start(page);await page.locator(".identity-menu-trigger").click();
 await page.getByRole("menuitem",{name:"Settings",exact:true}).click();
 const settings=page.locator(".settings-sidebar"),trigger=settings.getByRole("button",{name:"Back to home",exact:true});
 await expect(trigger).toBeVisible();await trigger.focus();await emitSearch(page);await expect(input(page)).toBeFocused();
 await page.keyboard.press("Escape");await expect(trigger).toBeFocused();await expect(page.locator(".memory-workspace-header input")).toHaveCount(0);
 await page.locator(".workspace-history-navigation").getByRole("button",{name:"Back",exact:true}).click();
 await openWikiNote(page,"Fixture architecture");const editor=page.getByRole("textbox",{name:"Page editor",exact:true});
 await editor.evaluate(el=>{const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});
 await page.keyboard.insertText("\n/");await expect(dialog(page)).toHaveCount(0);expect(await source(editor)).toContain("\n/");
});

test("empty search offers actual recently opened pages and Spaces, and clearing restores them",async({page})=>{
 await start(page);await emitSearch(page);await expect(dialog(page).getByText("Recently opened",{exact:true})).toHaveCount(0);await page.keyboard.press("Escape");
 await openWikiNote(page,"Fixture architecture");
 await expect.poll(()=>page.evaluate(()=>JSON.parse(localStorage.getItem('wenlan:recent-pages:v1')??'{"entries":[]}').entries.length)).toBe(1);
 await page.getByRole('navigation',{name:'Primary navigation'}).getByRole('button',{name:'Spaces',exact:true}).click();
 await page.getByRole('button',{name:'Open Research',exact:true}).click();
 await expect(page.getByRole('heading',{name:'Research',exact:true})).toBeVisible();
 await emitSearch(page);await expect(dialog(page).getByText("Recently opened",{exact:true})).toBeVisible();
 await expect(dialog(page).getByRole('button',{name:/Fixture architecture/})).toBeVisible();
 await expect(dialog(page).getByRole('button',{name:/Research/})).toBeVisible();
 await input(page).fill('fixture');await expect(dialog(page).getByText("Recently opened",{exact:true})).toHaveCount(0);
 await input(page).fill('');await expect(dialog(page).getByRole('button',{name:/Fixture architecture/})).toBeVisible();
 await dialog(page).getByRole('button',{name:/Fixture architecture/}).click();
 await expect(dialog(page)).toHaveCount(0);await expect(page.getByRole('tab',{name:'Fixture architecture',exact:true})).toHaveAttribute('aria-selected','true');
});
