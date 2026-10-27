// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from '@playwright/test';
import { collectBrowserErrors, installTauriMock } from './tauriMock';
import { openPrimaryDestination } from './helpers/primaryNavigation';

async function capture(page: Page, file: string) {
  const directory = process.env.WENLAN_UI_EVIDENCE_DIR;
  if (!directory) return;
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter(a => Number.isFinite(a.effect?.getComputedTiming().endTime)).map(a => a.finished.catch(() => undefined)));
  });
  await page.screenshot({ path: `${directory}/${file}.png` });
}

for (const theme of ['light', 'dark']) {
  test(`settings navigation and embedded connection fit the workspace in ${theme}`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width: 1280, height: 900 });
    await installTauriMock(page, { locale: 'en', rawActions: [], agents: [], localStorage: { 'wenlan-theme': theme } });
    await page.goto('/');
    await page.locator('.memory-sidebar').getByRole('button', { name: 'Account menu' }).click();
    await page.getByRole('menuitem', { name: 'Settings', exact: true }).click();
    const sidebar = page.locator('.settings-sidebar');
    const toggle = page.locator('[data-sidebar-toggle]');
    const main = page.locator('.memory-shell-content > main');
    await expect(main.getByRole('heading', { level: 1, name: 'General', exact: true })).toBeVisible();
    await expect(sidebar).toHaveCSS('width', '240px');
    expect((await page.locator('.workspace-header-sidebar-backdrop').boundingBox())!.width).toBe(240);
    await toggle.press('Enter');
    await expect(sidebar).toHaveAttribute('inert', '');
    await expect(main).toHaveCSS('width', '1280px');
    await toggle.press('Enter');
    await expect(sidebar).not.toHaveAttribute('inert', '');
    await page.setViewportSize({ width: 375, height: 900 });
    await expect(sidebar).toBeHidden();
    await expect(main).toHaveCSS('width', '375px');
    await toggle.press('Enter');
    await expect(sidebar).toBeVisible();
    const home = sidebar.getByRole('button', { name: 'Back to home', exact: true });
    const general = sidebar.getByRole('button', { name: 'General', exact: true });
    const privacy = sidebar.getByRole('link', { name: 'Privacy policy', exact: true });
    await expect(home).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(general).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    await expect(home).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    // The privacy link is the last focusable item inside the overlay trap.
    await expect(privacy).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(home).toBeFocused();
    await capture(page, `settings-drawer-${theme}`);
    await page.keyboard.press('Escape');
    await expect(sidebar).toBeHidden();await expect(toggle).toBeFocused();
    for (const label of ['General','Intelligence','Diagnostics','Connections','Sources']) {
      await toggle.press('Enter');
      await sidebar.getByRole('button', { name: label, exact: true }).press('Enter');
      await expect(sidebar).toBeHidden();await expect(toggle).toBeFocused();
      await expect(main.getByRole('heading', { level: 1, name: label, exact: true })).toBeVisible();
      await expect(main).toHaveCSS('width', '375px');
      expect(await main.evaluate(e => e.scrollWidth-e.clientWidth)).toBeLessThanOrEqual(1);
      await capture(page, `settings-${label}-${theme}-375`);
    }
    await toggle.press('Enter');
    await sidebar.getByRole('button', { name: 'Connections', exact: true }).press('Enter');
    await expect(main.getByRole('heading', { level: 3, name: 'Connected', exact: true })).toBeVisible();
    expect(await main.evaluate(e => e.scrollWidth-e.clientWidth)).toBeLessThanOrEqual(1);
    await toggle.press('Enter');
    await sidebar.getByRole('button', { name: 'Back to home', exact: true }).press('Enter');
    await toggle.press('Enter');
    await page.locator('.memory-sidebar').getByRole('button', { name: 'Account menu' }).click();
    await page.getByRole('menuitem', { name: 'What your AI can do', exact: true }).click();
    const askAi = page.getByRole('dialog', { name: 'What your AI can do', exact: true });
    await expect(askAi).toBeVisible();
    await askAi.getByRole('button', { name: 'Connect a tool', exact: true }).click();
    await expect(page.getByTestId('wizard-action-bar')).toBeInViewport();
    await expect(toggle).toHaveCount(0);
    const bar = await page.getByTestId('wizard-action-bar').boundingBox();
    expect(bar!.y+bar!.height).toBeLessThanOrEqual(900);
    await capture(page, `connect-embedded-${theme}-375`);
    await page.setViewportSize({ width:1280,height:900 });
    await expect(page.getByTestId('wizard-action-bar')).toBeInViewport();
    await expect(toggle).toHaveCount(0);
    await capture(page, `connect-embedded-${theme}-1280`);
    expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
  });

  test(`global graph uses full workspace pane and narrow modal in ${theme}`, async ({ page }) => {
    const errors=collectBrowserErrors(page);
    await page.setViewportSize({width:1280,height:900});
    await installTauriMock(page,{locale:'en',rawActions:[],localStorage:{'wenlan-theme':theme}});
    await page.goto('/');await openPrimaryDestination(page,'Graph');
    const search=page.getByRole('combobox',{name:'Filter nodes'});
    await search.fill('Ada');await page.getByRole('option',{name:/Ada Lovelace/}).first().click();
    const panel=page.getByRole('complementary',{name:'Ada Lovelace',exact:true});
    await expect(panel).toBeVisible();
    const box=await panel.boundingBox();const host=await page.locator('.workspace-pane-host').boundingBox();const mainBox=await page.locator('.memory-shell-content > main').boundingBox();
    expect(box!.y).toBe(0);expect(box!.height).toBe(900);expect(box!.x).toBe(host!.x);expect(box!.width).toBe(host!.width);
    expect(box!.x).toBeGreaterThanOrEqual(mainBox!.x+mainBox!.width-1);expect(box!.x+box!.width).toBe(1280);
    await expect(panel).toHaveCSS('background-color',theme==='light'?'rgb(252, 252, 251)':'rgb(25, 28, 36)');
    await expect(panel).toHaveCSS('border-top-left-radius','12px');
    await capture(page,`atlas-pane-${theme}-1280`);
    await panel.getByRole('button',{name:'Return to full map',exact:true}).press('Escape');
    await expect(panel).toHaveCount(0);await expect(search).toBeFocused();
    await expect(page.locator('.memory-workspace-header')).toHaveCSS('width','1280px');
    await page.setViewportSize({width:375,height:900});
    await search.fill('Ada');await page.getByRole('option',{name:/Ada Lovelace/}).first().click();
    const modal=page.getByRole('dialog',{name:'Ada Lovelace',exact:true});
    await expect(modal).toBeVisible();await expect(modal).toHaveAttribute('aria-modal','true');
    await expect(modal.getByRole('button',{name:'Return to full map',exact:true})).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    expect(await modal.evaluate(e=>e.contains(document.activeElement))).toBe(true);
    await capture(page,`atlas-pane-${theme}-375`);
    await page.keyboard.press('Escape');await expect(modal).toHaveCount(0);await expect(search).toBeFocused();
    expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
  });
}
