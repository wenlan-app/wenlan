// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RECENT_PAGES_STORAGE_KEY } from "../../../lib/recentPages";
import { deletePage, getPage, renamePage, type Page } from "../../../lib/tauri";
import { RecentNotesPanel } from "./RecentNotesPanel";

vi.mock("../../../lib/tauri", async original => ({
  ...await original<typeof import("../../../lib/tauri")>(),
  deletePage: vi.fn(),
  getPage: vi.fn(),
  renamePage: vi.fn(),
}));

const preferenceKey = "wenlan:wiki-inventory:v1";
const recent = [
  { id: "a", title: "Alpha", visitedAt: Date.now() },
  { id: "b", title: "Beta", visitedAt: Date.now() - 1 },
  { id: "c", title: "Gamma", visitedAt: Date.now() - 2 },
];

function rowTitles() {
  return [...document.querySelectorAll(".wiki-recent-note")].map(node => node.textContent);
}

function rect(top: number, bottom: number): DOMRect {
  return { x: 0, y: top, top, bottom, left: 0, right: 200, width: 200, height: bottom - top, toJSON: () => ({}) } as DOMRect;
}

describe("RecentNotesPanel custom ordering", () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem(RECENT_PAGES_STORAGE_KEY, JSON.stringify({ version: 1, entries: recent }));
    localStorage.setItem(preferenceKey, JSON.stringify({ version: 1, mode: "custom", customOrder: ["a", "b", "c"] }));
  });

  it("keeps a selected lower note in place across refreshes and appends a new recent entry", () => {
    const { rerender } = render(<RecentNotesPanel mode="custom" currentPageId="c" revision={1} onOpenPage={vi.fn()} />);
    expect(rowTitles()).toEqual(["Alpha", "Beta", "Gamma"]);
    expect(screen.getByRole("button", { name: "Gamma" })).toHaveAttribute("aria-current", "page");
    localStorage.setItem(RECENT_PAGES_STORAGE_KEY, JSON.stringify({ version: 1, entries: [
      { ...recent[0], visitedAt: Date.now() - 100 }, recent[1], recent[2], { id: "d", title: "Delta", visitedAt: Date.now() },
    ] }));
    rerender(<RecentNotesPanel mode="custom" currentPageId="c" revision={2} onOpenPage={vi.fn()} />);
    expect(rowTitles()).toEqual(["Alpha", "Beta", "Gamma", "Delta"]);
  });

  it("commits pointer reordering on release and restores the original order on cancellation", () => {
    const bounds = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("wiki-recent-notes-list")) return rect(0, 126);
      if (this.tagName === "LI") {
        const title = this.querySelector(".wiki-recent-note")?.textContent;
        const top = title === "Alpha" ? 0 : title === "Beta" ? 42 : 84;
        return rect(top, top + 40);
      }
      return rect(0, 40);
    });
    const { unmount } = render(<RecentNotesPanel mode="custom" onOpenPage={vi.fn()} />);
    const alphaGrip = screen.getByRole("button", { name: "Drag to reorder Alpha" });
    fireEvent.pointerDown(alphaGrip, { pointerId: 1, isPrimary: true, button: 0, clientY: 20 });
    fireEvent.pointerMove(window, { pointerId: 1, clientY: 65 });
    fireEvent.pointerUp(window, { pointerId: 1, clientY: 65 });
    expect(rowTitles()).toEqual(["Beta", "Alpha", "Gamma"]);
    expect(JSON.parse(localStorage.getItem(preferenceKey) ?? "{}").customOrder).toEqual(["b", "a", "c"]);
    unmount();

    localStorage.setItem(preferenceKey, JSON.stringify({ version: 1, mode: "custom", customOrder: ["a", "b", "c"] }));
    render(<RecentNotesPanel mode="custom" onOpenPage={vi.fn()} />);
    fireEvent.pointerDown(screen.getByRole("button", { name: "Drag to reorder Gamma" }), { pointerId: 2, isPrimary: true, button: 0, clientY: 104 });
    fireEvent.pointerMove(window, { pointerId: 2, clientY: 55 });
    fireEvent.pointerUp(window, { pointerId: 2, clientX: 220, clientY: 55 });
    expect(rowTitles()).toEqual(["Alpha", "Beta", "Gamma"]);

    fireEvent.pointerDown(screen.getByRole("button", { name: "Drag to reorder Gamma" }), { pointerId: 3, isPrimary: true, button: 0, clientY: 104 });
    fireEvent.pointerMove(window, { pointerId: 3, clientY: 55 });
    fireEvent(window, new PointerEvent("lostpointercapture", { pointerId: 3 }));
    expect(rowTitles()).toEqual(["Alpha", "Beta", "Gamma"]);
    bounds.mockRestore();
  });

  it("reorders with arrow keys without navigating or changing the selected page", () => {
    const onOpenPage = vi.fn();
    render(<RecentNotesPanel mode="custom" currentPageId="c" onOpenPage={onOpenPage} />);
    fireEvent.keyDown(screen.getByRole("button", { name: "Drag to reorder Beta" }), { key: "ArrowUp" });
    expect(rowTitles()).toEqual(["Beta", "Alpha", "Gamma"]);
    expect(screen.getByRole("button", { name: "Gamma" })).toHaveAttribute("aria-current", "page");
    expect(onOpenPage).not.toHaveBeenCalled();
  });
});

describe("RecentNotesPanel empty list", () => {
  beforeEach(() => localStorage.clear());

  it("offers the folder view when no note has been opened yet", () => {
    const onBrowseFolders = vi.fn();
    render(<RecentNotesPanel onBrowseFolders={onBrowseFolders} />);
    fireEvent.click(screen.getByRole("button", { name: "Browse folders" }));
    expect(onBrowseFolders).toHaveBeenCalledTimes(1);
  });
});

describe("RecentNotesPanel page menu", () => {
  const page = (id: string, title: string, overrides: Partial<Page> = {}): Page => ({
    id, title, summary: null, content: "", entity_id: null, domain: null, source_memory_ids: [], version: 3,
    status: "active", created_at: "2026-10-29", last_modified: "2026-10-29", last_compiled: "2026-10-29",
    creation_kind: "authored", storage_path: `${id}.md`, ...overrides,
  });
  const pages: Record<string, Page> = { a: page("a", "Alpha"), b: page("b", "Beta"), c: page("c", "Gamma") };
  const storedTitles = () => (JSON.parse(localStorage.getItem(RECENT_PAGES_STORAGE_KEY) ?? "{}").entries as { title: string }[]).map(entry => entry.title);
  function setup(props: Partial<React.ComponentProps<typeof RecentNotesPanel>> = {}) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><RecentNotesPanel onOpenPage={vi.fn()} {...props} /></QueryClientProvider>);
    return userEvent.setup();
  }

  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem(RECENT_PAGES_STORAGE_KEY, JSON.stringify({ version: 1, entries: recent }));
    vi.mocked(getPage).mockReset().mockImplementation(async id => pages[id] ?? null);
    vi.mocked(deletePage).mockReset().mockResolvedValue();
    vi.mocked(renamePage).mockReset();
  });

  it("right-clicking a row opens the page menu instead of the webview menu", async () => {
    const user = setup();
    const row = screen.getByRole("button", { name: "Beta" });
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 60 });
    row.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map(item => item.textContent))
      .toEqual(["Open", "Rename page", "Move to folder", "Remove from Recently used", "Delete page"]);
    expect(within(menu).getByRole("menuitem", { name: "Open" })).toHaveFocus();
    await waitFor(() => expect(within(menu).getByRole("menuitem", { name: "Rename page" })).toHaveAttribute("aria-disabled", "false"));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(row).toHaveFocus();
  });

  it("opens from the keyboard and opens the page", async () => {
    const onOpenPage = vi.fn();
    const user = setup({ onOpenPage });
    screen.getByRole("button", { name: "Gamma" }).focus();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    await user.click(await screen.findByRole("menuitem", { name: "Open" }));
    expect(onOpenPage).toHaveBeenCalledWith("c");
  });

  it("removes a page from the list without deleting it", async () => {
    const user = setup();
    await user.pointer({ keys: "[MouseRight]", target: screen.getByRole("button", { name: "Alpha" }) });
    await user.click(await screen.findByRole("menuitem", { name: "Remove from Recently used" }));
    expect(rowTitles()).toEqual(["Beta", "Gamma"]);
    expect(storedTitles()).toEqual(["Beta", "Gamma"]);
    expect(deletePage).not.toHaveBeenCalled();
  });

  it("deletes after confirmation, drops the row and tells the host", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const onDeletePage = vi.fn();
    const user = setup({ onDeletePage });
    await user.pointer({ keys: "[MouseRight]", target: screen.getByRole("button", { name: "Beta" }) });
    const item = await screen.findByRole("menuitem", { name: "Delete page" });
    await waitFor(() => expect(item).toHaveAttribute("aria-disabled", "false"));
    await user.click(item);
    expect(confirmSpy).toHaveBeenCalledWith("Delete this page?");
    await waitFor(() => expect(onDeletePage).toHaveBeenCalledWith("b"));
    expect(deletePage).toHaveBeenCalledWith("b");
    expect(rowTitles()).toEqual(["Alpha", "Gamma"]);
    confirmSpy.mockRestore();
  });

  it("renames in place at the page's latest version", async () => {
    vi.mocked(renamePage).mockResolvedValue({ id: "a", title: "Alpha 2", version: 4 });
    const user = setup();
    await user.pointer({ keys: "[MouseRight]", target: screen.getByRole("button", { name: "Alpha" }) });
    const item = await screen.findByRole("menuitem", { name: "Rename page" });
    await waitFor(() => expect(item).toHaveAttribute("aria-disabled", "false"));
    await user.click(item);
    const input = screen.getByRole("textbox", { name: "Rename page" });
    await user.clear(input);
    await user.type(input, "Alpha 2{Enter}");
    await waitFor(() => expect(rowTitles()).toEqual(["Alpha 2", "Beta", "Gamma"]));
    expect(renamePage).toHaveBeenCalledWith("a", "Alpha 2", 3);
    expect(storedTitles()[0]).toBe("Alpha 2");
  });

  it("keeps the old title and says so when a rename fails", async () => {
    vi.mocked(renamePage).mockRejectedValue(new Error("page_title_conflict"));
    const user = setup();
    await user.pointer({ keys: "[MouseRight]", target: screen.getByRole("button", { name: "Alpha" }) });
    const item = await screen.findByRole("menuitem", { name: "Rename page" });
    await waitFor(() => expect(item).toHaveAttribute("aria-disabled", "false"));
    await user.click(item);
    const input = screen.getByRole("textbox", { name: "Rename page" });
    await user.clear(input);
    await user.type(input, "Beta{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not rename this page");
    await user.keyboard("{Escape}");
    expect(rowTitles()).toEqual(["Alpha", "Beta", "Gamma"]);
  });
});
