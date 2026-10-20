// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page, type Locator } from '@playwright/test';
import { installTauriMock, collectBrowserErrors } from './tauriMock';
import { createSpacesNavigationFixture } from './fixtures/spacesNavigation';
async function setup(page: Page, delay = 0) {
    const f = createSpacesNavigationFixture();
    const base = f.spaces[0];
    const fixture = { ...f, spaces: [{ ...base, id: 'star', name: 'Starred', starred: true, sort_order: 0 }, ...['Alpha', 'Bravo', 'Charlie'].map((name, i) => ({ ...base, id: name.toLowerCase(), name, starred: false, sort_order: i + 1, description: i === 1 ? 'A deliberately longer description that wraps in the narrow layout so dragging also accounts for rows with different heights.' : 'Pages and project references.' }))], pages: f.pages.map((p, i) => ({ ...p, space: i < 2 ? 'Alpha' : 'Bravo' })) };
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale: 'en', fixture, rawActions: [], delays: { reorder_space: delay }, localStorage: { 'wenlan-theme': 'dark', 'wenlan-spaces-view-mode': 'rows' } });
    await page.goto('/');
    if (page.viewportSize()!.width < 700)
        await page.locator('[data-sidebar-toggle]').click();
    await page.getByRole('navigation', { name: 'Primary navigation' }).getByRole('button', { name: 'Spaces', exact: true }).click();
    await expect(page.getByTestId('space-row-alpha')).toBeVisible();
    return { controller, errors };
}
const row = (page: Page, id: string) => page.getByTestId(`space-row-${id}`);
async function box(el: Locator) { return (await el.boundingBox())!; }
async function begin(page: Page, id: string) { const b = await box(row(page, id).locator('.spaces-drag-handle')); const x = b.x + b.width / 2, y = b.y + b.height / 2; await page.mouse.move(x, y); await page.mouse.down(); return { x, y }; }
async function order(page: Page) { return page.locator('.spaces-confirmed-section .spaces-row[data-testid]').evaluateAll(es => es.map(e => e.getAttribute('data-testid'))); }
for (const width of [1280, 375])
    test(`row follows pointer and makes room before one saved drop ${width}`, async ({ page }, info) => {
        await page.setViewportSize({ width, height: 960 });
        const { controller, errors } = await setup(page, 450);
        const a = await box(row(page, 'alpha')), b = await box(row(page, 'bravo')), c = await box(row(page, 'charlie'));
        const pos = await begin(page, 'alpha');
        await page.mouse.move(pos.x, pos.y + 12, { steps: 3 });
        expect((await box(row(page, 'alpha'))).y - a.y).toBeGreaterThan(6);
        // The lifted row is opaque immediately; other rows must not show through.
        expect(await row(page, 'alpha').evaluate(e => getComputedStyle(e).backgroundColor)).toBe('rgb(32, 36, 48)');
        await page.mouse.move(pos.x, c.y + c.height / 2, { steps: 14 });
        await expect.poll(async () => (await box(row(page, 'bravo'))).y).toBeLessThan(b.y - 15);
        expect(controller.calls().filter(c => c.command === 'reorder_space')).toHaveLength(0);
        await page.screenshot({ path: info.outputPath('drag-in-progress.png') });
        const lifted = await box(row(page, 'alpha'));
        await page.evaluate(() => { const samples: number[] = []; (window as Window & {
            __dropFrames?: number[];
        }).__dropFrames = samples; const start = performance.now(); const sample = () => { const e = document.querySelector('[data-testid="space-row-alpha"]'); if (e)
            samples.push(e.getBoundingClientRect().top); if (performance.now() - start < 900)
            requestAnimationFrame(sample); }; requestAnimationFrame(sample); });
        await page.mouse.up();
        // The projected position stays visible during the pending write, instead of flashing back.
        expect((await box(row(page, 'alpha'))).y).toBeGreaterThan(a.y + 20);
        await expect.poll(() => controller.calls().filter(c => c.command === 'reorder_space').length).toBe(1);
        await expect.poll(() => order(page)).toEqual(['space-row-star', 'space-row-bravo', 'space-row-charlie', 'space-row-alpha']);
        await expect(row(page, 'alpha')).toHaveAttribute('aria-busy', 'false');
        await page.evaluate(() => new Promise<void>(resolve => setTimeout(resolve, 220)));
        const samples = await page.evaluate(() => (window as Window & {
            __dropFrames?: number[];
        }).__dropFrames as number[]);
        expect(Math.max(...samples), 'drop should not overshoot when canonical order replaces the preview').toBeLessThanOrEqual(Math.max(lifted.y, c.y + c.height - a.height) + 12);
        await expect(page.locator('.spaces-table-head')).toHaveCount(0);
        await expect(row(page, 'alpha')).toContainText('2 pages');
        await page.screenshot({ path: info.outputPath('after-drop.png') });
        expect(errors.pageErrors).toEqual([]);
        expect(errors.consoleErrors).toEqual([]);
        await info.attach('drag-evidence', { body: JSON.stringify({ before: { a, b, c }, lifted, calls: controller.calls().filter(c => c.command === 'reorder_space') }), contentType: 'application/json' });
    });
test('cancel, outside release, and starred boundary never save a drag', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 960 });
    const { controller, errors } = await setup(page);
    const original = await order(page);
    const target = await box(row(page, 'charlie'));
    let p = await begin(page, 'alpha');
    await page.mouse.move(p.x, target.y + target.height / 2, { steps: 10 });
    await page.keyboard.press('Escape');
    await page.mouse.up();
    expect(await order(page)).toEqual(original);
    p = await begin(page, 'alpha');
    await page.mouse.move(p.x, target.y + target.height / 2, { steps: 10 });
    await page.mouse.move(10, 10);
    await page.mouse.up();
    expect(await order(page)).toEqual(original);
    const star = await box(row(page, 'star'));
    p = await begin(page, 'alpha');
    await page.mouse.move(p.x, star.y + star.height / 2, { steps: 10 });
    await page.mouse.up();
    expect(await order(page)).toEqual(original);
    expect(controller.calls().filter(c => c.command === 'reorder_space')).toHaveLength(0);
    expect(errors.pageErrors).toEqual([]);
});
test('a rejected drop restores canonical order and keyboard reordering still works', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 960 });
    const { controller, errors } = await setup(page, 300);
    const original = await order(page);
    controller.failNext('reorder_space', 'Synthetic reorder failure');
    const target = await box(row(page, 'charlie'));
    const p = await begin(page, 'alpha');
    await page.mouse.move(p.x, target.y + target.height / 2, { steps: 10 });
    await page.mouse.up();
    await expect(page.getByText('The change could not be saved', { exact: true })).toBeVisible();
    expect(await order(page)).toEqual(original);
    await expect(row(page, 'alpha')).toHaveAttribute('aria-busy', 'false');
    await row(page, 'alpha').locator('.spaces-drag-handle').focus();
    await page.keyboard.press('ArrowDown');
    await expect.poll(() => order(page)).toEqual(['space-row-star', 'space-row-bravo', 'space-row-alpha', 'space-row-charlie']);
    expect(controller.calls().filter(c => c.command === 'reorder_space')).toHaveLength(2);
    expect(errors.pageErrors).toEqual([]);
});
test('upward filtered dragging and reduced motion keep the same saving contract', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 960 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const { controller, errors } = await setup(page);
    await page.getByLabel('Filter spaces').fill('Pages and project');
    await expect(row(page, 'bravo')).toHaveCount(0);
    const target = await box(row(page, 'alpha'));
    const p = await begin(page, 'charlie');
    const draggedRow = await box(row(page, 'charlie'));
    const grabOffsetY = p.y - draggedRow.y;
    const liftedRowCenterTarget = target.y + target.height / 2;
    await page.mouse.move(p.x, liftedRowCenterTarget + grabOffsetY - draggedRow.height / 2, { steps: 12 });
    await page.mouse.up();
    await expect.poll(() => order(page)).toEqual(['space-row-charlie', 'space-row-alpha']);
    expect(controller.calls().filter(c => c.command === 'reorder_space')).toHaveLength(1);
    expect(errors.pageErrors).toEqual([]);
});
