import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

function stylesheet(path: string): string {
  return readFileSync(resolve(path), "utf8");
}

describe("Wiki, Space, and Page microcontrol contract", () => {
  it("defines the shared control, icon action, and popover tokens", () => {
    const css = stylesheet("src/index.css");

    expect(css).toMatch(/--mem-control-height:\s*34px;/);
    expect(css).toMatch(/--mem-control-radius:\s*6px;/);
    expect(css).toMatch(/--mem-control-font-size:\s*14px;/);
    expect(css).toMatch(/--mem-icon-action-size:\s*32px;/);
    expect(css).toMatch(/--mem-icon-glyph-size:\s*16px;/);
    expect(css).toMatch(/--mem-popover-radius:\s*6px;/);
    expect(css).toMatch(/--mem-popover-shadow:\s*var\(--mem-shadow-overlay\);/);
  });

  it("keeps the established layout and typography invariants", () => {
    const root = stylesheet("src/index.css");
    const navigation = stylesheet("src/components/memory/navigation/navigation-shell.css");
    const draft = stylesheet("src/components/memory/pages/pageDraftEditor.css");
    const spaces = stylesheet("src/components/memory/spaces/spaces.css");

    expect(root).toMatch(/--mem-font-heading:\s*'Fraunces'/);
    expect(root).toMatch(/--mem-font-body:\s*'Instrument Sans'/);
    expect(root).toMatch(/--mem-destination-title-size:\s*34px;/);
    expect(navigation).toMatch(/\.memory-main-content\s*\{[^}]*padding:\s*56px 72px 28px;/s);
    expect(navigation).not.toMatch(/\.wiki-table td\s*\{[^}]*height:\s*\d+px;/s);
    expect(navigation).toMatch(/\.wiki-table td\s*\{[^}]*vertical-align:\s*middle;/s);
    expect(spaces).toMatch(/\.spaces-row\s*\{[^}]*min-height:\s*64px;/s);
    expect(root).toMatch(/\.page-detail-prose\s*\{[^}]*max-width:\s*730px;/s);
    expect(root).toMatch(
      /\.page-detail-title\s*\{[^}]*font-size:\s*clamp\(24px, 1\.6vw \+ 14px, 30px\);/s,
    );
    expect(draft).toMatch(/\.page-draft-editor\s*\{[^}]*margin-inline:\s*auto;/s);
    // A new page uses the saved page's reading column (readingSurface.css).
    expect(draft).toMatch(/\.page-draft-editor\s*\{[^}]*width:\s*min\(var\(--reading-frame-max-width\), 100%\);/s);
    expect(draft).toMatch(/\.page-draft-editor-axis\s*\{[^}]*max-width:\s*var\(--reading-measure\);/s);
  });

  it("applies one shared visual contract to text controls, icon actions, and menus", () => {
    const root = stylesheet("src/index.css");
    const navigation = stylesheet("src/components/memory/navigation/navigation-shell.css");
    const actions = stylesheet("src/components/memory/pages/pageActions.css");
    const draft = stylesheet("src/components/memory/pages/pageDraftEditor.css");
    const spaces = stylesheet("src/components/memory/spaces/spaces.css");
    const dossier = stylesheet("src/components/memory/space-detail/space-detail-header.css");

    expect(root).toMatch(/\.mem-icon-action\s*\{[^}]*width:\s*var\(--mem-icon-action-size\);[^}]*height:\s*var\(--mem-icon-action-size\);/s);
    expect(root).toMatch(/\.mem-icon-action svg\s*\{[^}]*width:\s*var\(--mem-icon-glyph-size\);[^}]*height:\s*var\(--mem-icon-glyph-size\);/s);
    expect(root).toMatch(/\.mem-popover-surface\s*\{[^}]*border-radius:\s*var\(--mem-popover-radius\);[^}]*box-shadow:\s*var\(--mem-popover-shadow\);/s);
    expect(actions).toMatch(/min-height:\s*var\(--mem-control-height\);/);
    expect(actions).toMatch(/border-radius:\s*var\(--mem-control-radius\);/);
    expect(draft).toMatch(/font:\s*500 var\(--mem-control-font-size\)\/1 var\(--mem-font-body\);/);
    // Wiki's compact inline filters use the approved 32px height.
    expect(navigation).toMatch(/\.wiki-filters select\s*\{[^}]*min-height:\s*32px;/s);
    expect(spaces).toMatch(/\.spaces-filter input,[\s\S]*?height:\s*var\(--mem-control-height\);/);
    expect(dossier).toMatch(
      /\.space-dossier-description\s*\{[^}]*font-size:\s*var\(--reading-font-size,\s*15px\);[^}]*line-height:\s*var\(--reading-line-height,\s*1\.75\);/s,
    );
    expect(spaces).toMatch(
      /\.spaces-primary-action,[\s\S]*?\.spaces-danger\s*\{[^}]*min-height:\s*var\(--mem-control-height\);[^}]*padding:\s*0 12px;/s,
    );
    expect(spaces).toMatch(
      /\.spaces-suggestion-action\s*\{[^}]*padding:\s*4px 10px;[^}]*font-size:\s*var\(--mem-text-control\);/s,
    );
    expect(dossier).toMatch(
      /\.space-dossier-suggestion-action\s*\{[^}]*border-radius:\s*var\(--mem-control-radius\);[^}]*font-size:\s*12px;[^}]*padding:\s*4px 10px;/s,
    );
  });

  it("stacks draft conflict actions instead of squeezing them on narrow screens", () => {
    const draft = stylesheet("src/components/memory/pages/pageDraftEditor.css");

    expect(draft).toMatch(
      /@media \(max-width: 639px\)[\s\S]*?\.page-draft-notice\s*\{[^}]*align-items:\s*flex-start;[^}]*flex-direction:\s*column;/s,
    );
    expect(draft).toMatch(
      /@media \(max-width: 639px\)[\s\S]*?\.page-draft-notice > div\s*\{[^}]*flex-wrap:\s*wrap;/s,
    );
  });

  it("owns the New Page copy in the tab creation action and Space toolbar", () => {
    const overview = readFileSync(
      resolve("src/components/memory/pages/NoteTabs.tsx"),
      "utf8",
    );
    const dossier = readFileSync(
      resolve("src/components/memory/space-detail/SpaceDossierHeader.tsx"),
      "utf8",
    );

    expect(overview).toContain('t("pages.overview.newPage")');
    expect(dossier).toContain('t("pages.overview.newPage")');
    expect(overview).not.toContain('t("pages.composer.dialogTitle")');
    expect(dossier).not.toContain('t("pages.composer.dialogTitle")');
  });
});
