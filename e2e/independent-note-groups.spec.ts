// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page, type Locator } from "@playwright/test";
import { showWikiFolders } from "./helpers/wikiWorkspace";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
const group = (page: Page, id: string) => page.locator(`[data-note-group-id="${id}"]`);
async function start(page: Page, theme = "light", width = 1440) {
 await page.setViewportSize({ width, height: 960 });
 const errors = collectBrowserErrors(page);
 const runtime = await installTauriMock(page, { locale: "en", rawActions: [], fixture: createReviewDecisionFixture("wiki-folders"), delays: { update_page: 250 }, localStorage: { "wenlan-theme": theme } });
 await page.goto("/"); await showWikiFolders(page);
 const tree = page.locator(".wiki-workspace-directory");
 await tree.getByRole("button", { name: "Work", exact: true }).click();
 await tree.getByRole("button", { name: "Open Fixture architecture", exact: true }).click();
 await expect(group(page,"primary").getByRole("textbox",{name:"Page editor",exact:true})).toBeVisible();
 await tree.getByRole("button", { name: "Research", exact: true }).click();
 await tree.getByRole("button", { name: "Open Why fixtures stay deterministic", exact: true }).click();
 await expect(group(page,"primary").getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 return { tree, runtime, errors };
}
async function move(page:Page, from="primary") {
 const label=from==="primary"?"Move to right group":"Move to central group";
 await group(page,from).getByRole("tab",{selected:true}).click({button:"right"});
 await page.getByRole("menuitem",{name:label,exact:true}).click();
}
async function append(page:Page, id:string, text:string) {
 const editor=group(page,id).getByRole("textbox",{name:"Page editor",exact:true});
 await editor.evaluate(el=>{const view=(el as any).cmTile.root.view;view.dispatch({selection:{anchor:view.state.doc.length}});view.focus();});
 await page.keyboard.insertText(text);return editor;
}
async function source(editor:Locator) { return editor.evaluate(el=>(el as any).cmTile.root.view.state.doc.toString() as string); }
test("inspector drag widths belong to each group and cancel without changing the other", async ({ page }) => {
 const { errors } = await start(page, "light", 2560);
 await move(page);
 const left = group(page, "primary"), right = group(page, "secondary");
 for (const frame of [left, right]) await frame.getByRole("button", { name: "Open page sidebar", exact: true }).click();
 const paneWidth = (frame: Locator) => frame.locator(".page-info-drawer-group-overlay-panel").evaluate(el => el.getBoundingClientRect().width);
 const preference = (id: string) => page.evaluate(k => localStorage.getItem(k), `wenlan-right-sidebar-v1:${id}`);
 await left.getByTestId("right-sidebar-resize-handle").focus();
 await page.keyboard.press("End");
 await expect.poll(() => paneWidth(left)).toBe(600);
 await expect.poll(() => paneWidth(right)).toBe(320);
 const savedLeft = await preference("primary"), beforeRight = await preference("secondary");
 const box = (await right.getByTestId("right-sidebar-resize-handle").boundingBox())!;
 const x = box.x + box.width / 2, y = box.y + box.height / 2;
 await page.mouse.move(x, y); await page.mouse.down(); await page.mouse.move(x - 80, y, { steps: 8 });
 await expect.poll(() => paneWidth(right)).toBe(400);
 expect(await preference("secondary")).toBe(beforeRight);
 await page.keyboard.press("Escape"); await page.mouse.up();
 await expect.poll(() => paneWidth(right)).toBe(320);
 await expect.poll(() => paneWidth(left)).toBe(600);
 await page.mouse.move(x, y); await page.mouse.down(); await page.mouse.move(x - 80, y, { steps: 8 }); await page.mouse.up();
 await expect.poll(() => paneWidth(right)).toBe(400);
 await expect.poll(() => preference("secondary")).not.toBe(beforeRight);
 expect(await preference("primary")).toBe(savedLeft);
 await right.getByRole("button", { name: "Close", exact: true }).click();
 await expect(left.getByRole("complementary")).toBeVisible();
 await right.getByRole("button", { name: "Open page sidebar", exact: true }).click();
 await expect.poll(() => paneWidth(right)).toBe(400);
 expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
test("independent editors, local inspectors, and a failed save cannot lose a moved page",async({page},info)=>{
 const {runtime,errors}=await start(page);
 await move(page);
 await expect(group(page,"secondary").getByRole("textbox",{name:"Page editor",exact:true})).toBeVisible();
 await expect(group(page,"primary").getByRole("tab",{name:"Fixture architecture",exact:true})).toBeVisible();
 const rightEditor=await append(page,"secondary","\nRight group edit remains.");
 await rightEditor.evaluate(el=>el.setAttribute("data-editor-instance","right"));
 await group(page,"primary").getByRole("button",{name:"Open page sidebar",exact:true}).click();
 await group(page,"secondary").getByRole("button",{name:"Open page sidebar",exact:true}).click();
 await expect(group(page,"primary").getByRole("tab",{name:"Info",exact:true})).toBeVisible();
 await expect(group(page,"secondary").getByRole("tab",{name:"Info",exact:true})).toBeVisible();
 // Closing an inspector in one group does not close the other.
 await group(page,"primary").getByRole("button",{name:"Close",exact:true}).click();
 await expect(group(page,"secondary").getByRole("tab",{name:"Info",exact:true})).toBeVisible();
 await expect(rightEditor).toHaveAttribute("data-editor-instance","right");
 await group(page,"secondary").getByRole("button",{name:"Close",exact:true}).click();
 await expect.poll(()=>source(rightEditor)).toContain("Right group edit remains.");
 await expect(group(page,"secondary").locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 runtime.failNext("update_page","save refused");
 await append(page,"secondary","\nUnsaved on failed move.");
 await move(page,"secondary");
 await expect(group(page,"secondary").locator(".page-detail").getByRole("alert")).toBeVisible();
 await expect(rightEditor).toHaveAttribute("data-editor-instance","right");
 await expect.poll(()=>source(rightEditor)).toContain("Unsaved on failed move.");
 await group(page,"secondary").getByRole("button",{name:"Retry",exact:true}).click();
 await expect(group(page,"secondary").locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 await page.screenshot({path:info.outputPath("split-notes.png"),fullPage:true});
 await move(page,"secondary");
 await expect(group(page,"secondary")).toHaveCount(0);
 await expect(group(page,"primary").getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
test("switching the central page preserves the right editor and both inspector choices",async({page},info)=>{
 const {tree,errors}=await start(page,"light",2560);
 await move(page);
 const right=group(page,"secondary"), left=group(page,"primary");
 await right.getByRole("button",{name:"Open page sidebar",exact:true}).click();
 await right.getByRole("tab",{name:"Mind map",exact:true}).click();
 await left.getByRole("button",{name:"Open page sidebar",exact:true}).click();
 const editor=right.locator(".page-detail");
 await editor.evaluate(el=>el.setAttribute("data-editor-instance","right-stays"));
 // Focus the central group before choosing another note in the shared tree.
 await left.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await tree.getByRole("button",{name:"Open Next experiment",exact:true}).click();
 await expect(left.getByRole("textbox",{name:"Title",exact:true})).toHaveValue("Next experiment");
 await expect(editor).toHaveAttribute("data-editor-instance","right-stays");
 await expect(right.getByRole("tab",{name:"Mind map",exact:true})).toHaveAttribute("aria-selected","true");
 await left.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await expect(left.getByRole("textbox",{name:"Page editor",exact:true})).toBeVisible();
 await page.screenshot({path:info.outputPath("independent-inspectors.png"),fullPage:true});
 expect(errors.pageErrors).toEqual([]);
});
test("a saved draft moves, auto-finalizes, and retains durable content in the right group",async({page})=>{
 const {tree}=await start(page);
 await tree.getByRole("button",{name:"Open Next experiment",exact:true}).click();
 const left=group(page,"primary");
 await left.getByRole("textbox",{name:"Content",exact:true}).fill("Draft transferred safely.");
 await move(page);
 const right=group(page,"secondary");
 const editor=right.getByRole("textbox",{name:"Page editor",exact:true});
 await expect(editor).toBeVisible();
 await expect(right.locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 await expect.poll(()=>source(editor)).toContain("Draft transferred safely.");
});

test("Back and Forward respect ownership and do not wait for the other group's failed save", async ({page}) => {
 const {tree,runtime}=await start(page);
 await group(page,"primary").getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await move(page); // A now belongs exclusively to the right.
 await expect(group(page,"secondary").getByRole("tab",{name:"Fixture architecture",exact:true})).toBeVisible();
 runtime.failNext("update_page","right save unavailable",4);
 const rightEditor=await append(page,"secondary","\nKeep failed right edit.");
 await expect(group(page,"secondary").locator(".page-detail").getByRole("alert")).toBeVisible();
 // Open an additional central page, then use the shared history buttons.
 await group(page,"primary").getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click();
 await tree.getByRole("button",{name:"Open Independent research",exact:true}).click();
 await expect(group(page,"primary").getByRole("tab",{name:"Independent research",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Back",exact:true}).click();
 await expect(group(page,"primary").getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Forward",exact:true}).click();
 await expect(group(page,"primary").getByRole("tab",{name:"Independent research",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Back",exact:true}).click();
 await page.getByRole("button",{name:"Back",exact:true}).click();
 await expect(group(page,"primary").getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveCount(0);
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveCount(1);
 await expect.poll(()=>source(rightEditor)).toContain("Keep failed right edit.");
});

test("Escape in a right draft cannot close or navigate a central draft", async ({page}) => {
 const {tree}=await start(page);
 await tree.getByRole("button",{name:"Open Next experiment",exact:true}).click();
 await move(page);
 const left=group(page,"primary"),right=group(page,"secondary");
 await left.getByRole("button",{name:"New page",exact:true}).click();
 const title=left.getByRole("textbox",{name:"Title",exact:true});
 await title.fill("Central draft stays");
 await right.getByRole("button",{name:"New page",exact:true}).click();
 const rightTitle=right.getByRole("textbox",{name:"Title",exact:true});
 await rightTitle.fill("Right draft on Escape");
 await right.getByRole("textbox",{name:"Content",exact:true}).fill("Right draft content.");
 await right.getByRole("textbox",{name:"Content",exact:true}).press("Escape");
 await expect(right.getByRole("tab",{name:"Right draft on Escape",exact:true})).toHaveCount(0);
 await expect(right.getByRole("tab",{name:"Next experiment",exact:true})).toHaveAttribute("aria-selected","true");
 await expect(title).toHaveValue("Central draft stays");
 await expect(left.getByRole("textbox",{name:"Content",exact:true})).toHaveValue("");
});

test("opening a page owned by the other group requires an explicit action", async ({page}) => {
 const {tree}=await start(page);
 await move(page);
 const left=group(page,"primary"),right=group(page,"secondary");
 await right.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click();
 await tree.getByRole("button",{name:"Open Independent research",exact:true}).click();
 await expect(right.getByRole("tab",{name:"Independent research",exact:true})).toHaveAttribute("aria-selected","true");
 await left.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await tree.getByRole("button",{name:"Open Why fixtures stay deterministic",exact:true}).click();
 await expect(right.getByRole("tab",{name:"Independent research",exact:true})).toHaveAttribute("aria-selected","true");
 await expect(left.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Show page",exact:true}).click();
 await expect(right.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveCount(1);
});

test("a newer right tab choice cancels a pending global graph navigation", async ({page}) => {
 const {tree}=await start(page,"light",2560);
 await move(page);
 const left=group(page,"primary"),right=group(page,"secondary");
 await right.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click();
 await tree.getByRole("button",{name:"Open Independent research",exact:true}).click();
 await right.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click();
 await right.getByRole("button",{name:"Open page sidebar",exact:true}).click();
 await append(page,"primary","\nSave before global graph navigation.");
 await right.getByRole("button",{name:"Open in graph",exact:true}).click();
 await right.getByRole("tab",{name:"Independent research",exact:true}).click();
 await expect(right.getByRole("tab",{name:"Independent research",exact:true})).toHaveAttribute("aria-selected","true");
 await expect(left.locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 await expect(page.locator(".atlas-view")).toHaveCount(0);
});
