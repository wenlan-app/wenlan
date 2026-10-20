import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { labels, makeSpace, renderOverview } from "./SpacesOverview.testUtils";

const api = vi.hoisted(() => ({
  listSpaces: vi.fn(),
  listPages: vi.fn(),
  createSpace: vi.fn(),
  updateSpace: vi.fn(),
  deleteSpace: vi.fn(),
  confirmSpace: vi.fn(),
  reorderSpace: vi.fn(),
  toggleSpaceStarred: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => api);

const confirmed = makeSpace({ id: "work", name: "Work", description: "A longer description remains visible in the compact list row." });
const suggested = makeSpace({ id: "suggested", name: "Suggested", suggested: true });
const desktopColumns = ["name", "menu"];

function directColumnNames(element: HTMLElement): string[] {
  return Array.from(element.children)
    .map((child) => child.getAttribute("data-space-column"))
    .filter((column): column is string => column !== null);
}

describe("SpacesOverview layout contract", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listSpaces.mockResolvedValue([confirmed, suggested]);
    api.listPages.mockResolvedValue([]);
  });

  it("keeps a compact self-labelled row with no visible column header", async () => {
    renderOverview();
    const inventory = await screen.findByRole("region", { name: labels.confirmedHeading });
    const row = screen.getByTestId("space-row-work");

    expect(within(inventory).queryByRole("row")).not.toBeInTheDocument();
    expect(within(inventory).queryByRole("columnheader")).not.toBeInTheDocument();
    expect(directColumnNames(row)).toEqual(desktopColumns);
    expect(row.querySelector('[data-space-column="updated"]')).toBeNull();
    expect(row.querySelector('[data-space-column="menu"]')).not.toBeNull();
    expect(row.querySelector('[data-space-column="drag"]')).toBeNull();
    expect(within(row).getByText(confirmed.description!)).toBeInTheDocument();
    const pageCount = within(row).getByTestId("space-pages");
    expect(pageCount).toHaveTextContent(/\d+ pages?/i);
    expect(row.querySelector('[data-space-column="pages"]')).toBeNull();
    expect(row.querySelector(".spaces-row-name")?.contains(pageCount)).toBe(true);
    expect(pageCount).not.toHaveAttribute("aria-hidden", "true");
    expect(pageCount).toBeVisible();
    expect(within(row).getByRole("button", { name: labels.dragSpace("Work") })).not.toHaveAttribute("title");

    const css = readFileSync(resolve("src/components/memory/spaces/spacesInventory.css"), "utf8");
    const baseCss = readFileSync(resolve("src/components/memory/spaces/spaces.css"), "utf8");
    expect(baseCss).toMatch(/\.spaces-overview\s*\{[^}]*max-width:\s*var\(--collection-reading-width,\s*960px\);/s);
    expect(css).not.toMatch(/spaces-table-head/);
    expect(css).toMatch(/white-space:\s*nowrap/);
    expect(css).toMatch(/grid-template-columns:\s*minmax\(0,\s*1fr\) 36px/);
    expect(css).toMatch(/\.spaces-drag-handle\s*\{[^}]*position:\s*absolute/);
    expect(css).toMatch(/\.spaces-drag-handle:hover,[\s\S]*?background:\s*transparent/);
    expect(css).toMatch(/\.spaces-row-main\s*\{[^}]*grid-template-columns:\s*20px minmax\(0,\s*1fr\)/);
    expect(css).toMatch(/min-height:\s*52px/);
    expect(css).toMatch(/@media \(pointer:\s*coarse\),\s*\(hover:\s*none\)[\s\S]*?spaces-drag-handle[^}]*width:\s*44px/);
  });

  it("keeps New space as the filled create action while Suggested decisions stay compact, tonal, and marker-free", async () => {
    renderOverview();
    const newSpace = screen.getByRole("button", { name: labels.newSpace });
    const suggestedRow = await screen.findByTestId("space-row-suggested");
    const keep = within(suggestedRow).getByRole("button", { name: labels.keep });
    const discard = within(suggestedRow).getByRole("button", { name: labels.discard });

    expect(newSpace).toHaveClass("spaces-new-action");
    expect(newSpace).toHaveClass("page-create-action");
    expect(newSpace).not.toHaveClass("spaces-primary-action");
    expect(newSpace.querySelector('[aria-hidden="true"]')).not.toBeInTheDocument();
    expect(keep).toHaveClass("spaces-suggestion-action", "spaces-suggestion-keep");
    expect(discard).toHaveClass("spaces-suggestion-action", "spaces-suggestion-discard");
    expect(keep).not.toHaveClass("spaces-primary-action");
    expect(discard).not.toHaveClass("spaces-primary-action");

    const css = readFileSync(resolve("src/components/memory/spaces/spacesInventory.css"), "utf8");
    const baseCss = readFileSync(resolve("src/components/memory/spaces/spaces.css"), "utf8");
    expect(css).not.toMatch(/\.spaces-row-suggested\s+\.spaces-row-main::before/);
    expect(css).toMatch(/\.spaces-overview-header \.spaces-new-action\s*\{[^}]*flex:\s*none/s);
    expect(css).not.toMatch(/\.spaces-overview-header \.spaces-new-action\s*\{[^}]*background:/s);
    expect(baseCss).not.toMatch(/\.spaces-new-action\s*\{/);
    expect(baseCss).not.toMatch(/\.spaces-new-action:hover/);
    expect(css).toMatch(/\.spaces-suggestion-action\s*\{[^}]*font-size:\s*var\(--mem-text-control\)[^}]*padding:\s*4px 10px/s);
    expect(baseCss).toMatch(/\.spaces-suggestion-keep\s*\{[^}]*background:\s*transparent[^}]*color:\s*var\(--mem-accent-indigo\)/s);
    expect(baseCss).toMatch(/\.spaces-suggestion-keep:hover\s*\{[^}]*background:\s*var\(--mem-indigo-bg\)/s);
  });

  it("keeps mobile row controls and the page count on one compact line", () => {
    const css = readFileSync(resolve("src/components/memory/spaces/spacesInventory.css"), "utf8");

    expect(css).toMatch(/\.spaces-overview \.spaces-row:not\(\.spaces-row-suggested\):not\(\.spaces-row-edit\)\s*\{[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\) 36px/);
    expect(css).toMatch(/\.spaces-overview \.spaces-row:not\(\.spaces-row-suggested\) \.spaces-row-pages\s*\{[^}]*display:\s*inline-block/);
    expect(css).toMatch(/@media \(pointer:\s*coarse\),\s*\(hover:\s*none\)[\s\S]*?grid-template-columns:\s*minmax\(0,\s*1fr\) 44px/);
    expect(css).toMatch(/\.spaces-overview \.spaces-row:not\(\.spaces-row-suggested\) \.spaces-menu-trigger\s*\{[^}]*width:\s*44px;[^}]*height:\s*44px/s);
  });

  it("compacts only sub-600 Suggested rows so owned Spaces enter the initial phone viewport", () => {
    const css = readFileSync(resolve("src/components/memory/spaces/spacesInventory.css"), "utf8");

    expect(css).toMatch(
      /@media \(max-width:\s*599px\)\s*\{[\s\S]*?\.spaces-overview \.spaces-row-suggested\s*\{[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\) auto;[^}]*min-height:\s*52px;[^}]*padding:\s*0;/,
    );
    expect(css).toMatch(
      /@media \(max-width:\s*599px\)\s*\{[\s\S]*?\.spaces-row-suggested \.spaces-row-main\s*\{[^}]*padding:\s*8px;/,
    );
    expect(css).toMatch(
      /@media \(max-width:\s*599px\)\s*\{[\s\S]*?\.spaces-row-suggested \.spaces-row-decisions\s*\{[^}]*padding:\s*0 4px 0 0;/,
    );
    expect(css).toMatch(/\.spaces-overview \.spaces-confirmed-section\s*\{[^}]*padding-top:\s*0;/);
  });

  it("gives the inline rename editor the full inventory width", async () => {
    renderOverview();
    fireEvent.click(await screen.findByRole("button", { name: labels.actionsFor("Work") }));
    fireEvent.click(screen.getByRole("menuitem", { name: labels.rename }));

    const row = screen.getByTestId("space-row-work");
    const editor = row.querySelector(":scope > .spaces-editor");
    expect(row).toHaveClass("spaces-row-edit");
    expect(editor).not.toBeNull();

    const css = readFileSync(resolve("src/components/memory/spaces/spacesInventory.css"), "utf8");
    const editRowRule = css.match(/\.spaces-overview \.spaces-row-edit\s*\{([^}]*)\}/)?.[1] ?? "";
    const editorRule = css.match(/\.spaces-row-edit\s*>\s*\.spaces-editor\s*\{([^}]*)\}/)?.[1] ?? "";

    expect(editRowRule).toMatch(/grid-template-columns:\s*minmax\(0,\s*1fr\)/);
    expect(editorRule).toMatch(/grid-column:\s*1 \/ -1/);
    expect(editorRule).toMatch(/min-width:\s*0/);
    expect(editorRule).toMatch(/width:\s*100%/);
  });
});
