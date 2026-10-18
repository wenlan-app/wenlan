// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RECENT_PAGES_STORAGE_KEY } from "../../../lib/recentPages";
import { RecentNotesPanel } from "./RecentNotesPanel";

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
