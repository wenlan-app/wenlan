// SPDX-License-Identifier: AGPL-3.0-only
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { NoteTabs } from "./NoteTabs";
import type { NoteTab } from "./noteTabState";

const tabs: NoteTab[] = [
  { key: "note:one", view: { kind: "page", pageId: "one" }, title: "First page" },
  { key: "note:two", view: { kind: "page", pageId: "two" }, title: "Second page" },
];

function renderTabs(overrides: Partial<React.ComponentProps<typeof NoteTabs>> = {}) {
  const props = {
    tabs,
    activeKey: tabs[0].key,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onCreate: vi.fn(),
    ...overrides,
  };
  return { ...render(<NoteTabs {...props} />), props };
}

describe("NoteTabs", () => {
  it("renders no home button unless the strip is given a home action", () => {
    renderTabs();

    expect(screen.queryByRole("button", { name: "Wiki home" })).not.toBeInTheDocument();
    expect(document.querySelector(".note-tab-home")).not.toBeInTheDocument();
  });

  it("puts a named home button before the tablist and reports whether home is the current place", async () => {
    const user = userEvent.setup();
    const onHome = vi.fn();
    const { rerender, props } = renderTabs({ onHome, homeActive: true, activeKey: undefined });
    const home = screen.getByRole("button", { name: "Wiki home" });

    expect(home).toHaveAttribute("aria-current", "page");
    expect(home).toHaveAttribute("data-active", "true");
    expect(document.querySelector(".note-tabs")?.firstElementChild).toBe(home);
    expect(home.compareDocumentPosition(screen.getByRole("tablist")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(home.closest('[role="tablist"]')).toBeNull();
    await user.click(home);
    expect(onHome).toHaveBeenCalledTimes(1);

    rerender(<NoteTabs {...props} onHome={onHome} homeActive={false} />);
    expect(screen.getByRole("button", { name: "Wiki home" })).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("button", { name: "Wiki home" })).toHaveAttribute("data-active", "false");
  });

  it("keeps the home button when no page is open and out of the tab arrow-key order", async () => {
    const user = userEvent.setup();
    renderTabs({ tabs: [], activeKey: undefined, onHome: vi.fn(), homeActive: true });
    expect(screen.getByRole("button", { name: "Wiki home" })).toBeInTheDocument();
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    cleanup();

    renderTabs({ onHome: vi.fn() });
    const first = screen.getByRole("tab", { name: "First page" });
    first.focus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Second page" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(first).toHaveFocus();
  });

  it("does not show a direct move action and marks its group and tabs for drag targeting", () => {
    const onMoveToOtherGroup = vi.fn();
    renderTabs({ onMoveToOtherGroup, moveLabel: "Move to other group", groupId: "secondary" });

    expect(document.querySelector(".note-tabs")).toHaveAttribute("data-note-tab-group", "secondary");
    expect(document.querySelector('[data-note-tab-key="note:one"]')).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open beside" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Move back" })).not.toBeInTheDocument();
  });

  it("opens the selected tab's action from the keyboard and restores tab focus on Escape", async () => {
    const user = userEvent.setup();
    const { props } = renderTabs({ onMoveToOtherGroup: vi.fn(), moveLabel: "Move to other group" });
    const tab = screen.getByRole("tab", { name: "First page" });
    tab.focus();

    await user.keyboard("{Shift>}{F10}{/Shift}");

    const action = await screen.findByRole("menuitem", { name: "Move to other group" });
    expect(action).toHaveFocus();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(tab).toHaveFocus());
    expect(props.onMoveToOtherGroup).not.toHaveBeenCalled();
  });

  it("opens from the pointer context menu and moves the tab that was invoked", async () => {
    const user = userEvent.setup();
    const onMoveToOtherGroup = vi.fn();
    const { props } = renderTabs({ onMoveToOtherGroup, moveLabel: "Move to other group" });
    const tab = screen.getByRole("tab", { name: "Second page" });

    fireEvent.contextMenu(tab, { clientX: 20, clientY: 30 });
    await user.click(await screen.findByRole("menuitem", { name: "Move to other group" }));

    expect(onMoveToOtherGroup).toHaveBeenCalledOnce();
    expect(onMoveToOtherGroup).toHaveBeenCalledWith(tabs[1]);
    expect(props.onSelect).not.toHaveBeenCalled();
  });

  it("uses a unique content target and supplied tab list label", () => {
    renderTabs({ contentId: "group-two-content", tabListLabel: "Pages in second group" });

    expect(screen.getByRole("tablist", { name: "Pages in second group" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "First page" })).toHaveAttribute("aria-controls", "group-two-content");
    expect(screen.getByRole("tab", { name: "Second page" })).toHaveAttribute("aria-controls", "group-two-content");
  });

  it("retains arrow navigation, close focus, and new tab behavior", async () => {
    const user = userEvent.setup();
    const { props, rerender } = renderTabs();
    const first = screen.getByRole("tab", { name: "First page" });
    const second = screen.getByRole("tab", { name: "Second page" });

    first.focus();
    await user.keyboard("{ArrowRight}");
    expect(second).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Close First page" }));
    expect(props.onClose).toHaveBeenCalledWith(tabs[0]);

    rerender(<NoteTabs {...props} tabs={[tabs[1]]} activeKey={tabs[1].key} />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "Second page" })).toHaveFocus());
    await user.click(screen.getByRole("button", { name: "New page" }));
    expect(props.onCreate).toHaveBeenCalledOnce();
  });
});

describe("NoteTabs window dragging", () => {
  it("lets the empty strip drag the window while tabs and buttons stay plain", () => {
    renderTabs({ onHome: vi.fn() });
    expect(document.querySelector(".note-tabs")).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByRole("tablist")).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByRole("tab", { name: "First page" })).not.toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByRole("button", { name: "Wiki home" })).not.toHaveAttribute("data-tauri-drag-region");
  });
});
