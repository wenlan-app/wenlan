// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { MemoryItem } from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  STABILITY_TIERS: { fact: "standard", preference: "protected" },
  getPendingRevision: vi.fn().mockResolvedValue(null),
  acceptPendingRevision: vi.fn().mockResolvedValue(undefined),
  dismissPendingRevision: vi.fn().mockResolvedValue(undefined),
}));

import MemoryListRow from "./MemoryListRow";
import { acceptPendingRevision, dismissPendingRevision, getPendingRevision } from "../../lib/tauri";

function makeMemory(overrides: Partial<MemoryItem> = {}): MemoryItem {
  return {
    source_id: "memory-1",
    title: "Generated title",
    content: "Keep the captured decision with its reasoning so the next session can continue.",
    summary: "Generated summary",
    source_text: "Imported source transcript has different wording.",
    memory_type: "fact",
    domain: "Wenlan",
    source_agent: "codex",
    confidence: null,
    confirmed: false,
    pinned: false,
    supersedes: null,
    last_modified: 100,
    chunk_count: 1,
    access_count: 0,
    is_recap: false,
    ...overrides,
  };
}

const callbacks = () => ({ onConfirm: vi.fn(), onDelete: vi.fn(), onPin: vi.fn(), onUnpin: vi.fn(), onClick: vi.fn() });

describe("MemoryListRow reading and operations", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getPendingRevision).mockResolvedValue(null);
  });

  it("leads with captured content and keeps the separate title accessible", () => {
    const memory = makeMemory();
    const { container } = render(<MemoryListRow memory={memory} {...callbacks()} />);

    expect(screen.getByRole("article", { name: memory.title! })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open memory" })).toHaveTextContent(memory.content);
    expect(screen.queryByText(memory.title!)).not.toBeInTheDocument();
    expect(screen.queryByText(memory.source_text!)).not.toBeInTheDocument();
    expect(screen.queryByText(memory.summary!)).not.toBeInTheDocument();
    expect(screen.getByText("Wenlan")).toBeVisible();
    expect(container.querySelector("dl")).toBeNull();
    expect(screen.queryByText("codex")).not.toBeInTheDocument();
    for (const label of ["Type", "Space", "Agent", "Status", "Updated"]) {
      expect(screen.queryByText(label)).not.toBeInTheDocument();
    }
  });

  it("does not add placeholder metadata when no space is present", () => {
    const { container } = render(<MemoryListRow memory={makeMemory({ domain: null })} {...callbacks()} />);
    expect(container.querySelector(".memory-list-row-context")).toBeNull();
    expect(screen.queryByText("—")).not.toBeInTheDocument();
  });

  it("opens the memory from content or the article keyboard without selecting it from controls", async () => {
    const props = callbacks();
    const user = userEvent.setup();
    render(<MemoryListRow memory={makeMemory()} {...props} />);

    await user.click(screen.getByRole("button", { name: "Open memory" }));
    const row = screen.getByRole("article");
    row.focus();
    await user.keyboard("{Enter} ");
    expect(props.onClick).toHaveBeenCalledTimes(3);
    expect(props.onClick).toHaveBeenLastCalledWith("memory-1");
    await user.click(screen.getByRole("button", { name: "Memory actions" }));
    await user.keyboard("{Enter}");
    expect(props.onClick).toHaveBeenCalledTimes(3);
    expect(props.onConfirm).toHaveBeenCalledWith("memory-1", true);
  });

  it("hides operations until the always available ellipsis is opened and returns focus after an action", async () => {
    const user = userEvent.setup();
    const props = callbacks();
    render(<MemoryListRow memory={makeMemory()} {...props} />);
    const trigger = screen.getByRole("button", { name: "Memory actions" });

    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Confirm memory" })).not.toBeInTheDocument();
    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("menuitem", { name: "Confirm memory" })).toHaveFocus();
    await user.click(screen.getByRole("menuitem", { name: "Pin memory" }));
    expect(props.onPin).toHaveBeenCalledWith("memory-1");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("supports keyboard opening, arrow navigation, Home/End and Escape", async () => {
    const user = userEvent.setup();
    const props = callbacks();
    const outerKeyDown = vi.fn();
    render(<div onKeyDown={outerKeyDown}><MemoryListRow memory={makeMemory({ confirmed: true, pinned: true })} {...props} /></div>);
    const trigger = screen.getByRole("button", { name: "Memory actions" });
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Unconfirm memory" })).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("menuitem", { name: "Delete memory" })).toHaveFocus();
    await user.keyboard("{Home}{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Unpin memory" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(props.onUnpin).toHaveBeenCalledWith("memory-1");
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("menuitem", { name: "Delete memory" })).toHaveFocus();
    outerKeyDown.mockClear();
    await user.keyboard("{Home}{End}{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(props.onDelete).not.toHaveBeenCalled();
    expect(outerKeyDown).not.toHaveBeenCalled();
  });

  it("dismisses the menu when focus leaves or the user clicks outside", async () => {
    const user = userEvent.setup();
    render(<><MemoryListRow memory={makeMemory()} {...callbacks()} /><button>Next control</button></>);
    const trigger = screen.getByRole("button", { name: "Memory actions" });
    await user.click(trigger);
    await user.keyboard("{End}{Tab}");
    expect(screen.getByRole("button", { name: "Next control" })).toHaveFocus();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await user.click(trigger);
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("preserves unconfirm and delete callbacks, including optimistic removal", async () => {
    const user = userEvent.setup();
    const props = callbacks();
    render(<MemoryListRow memory={makeMemory({ confirmed: true })} {...props} />);
    await user.click(screen.getByRole("button", { name: "Memory actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Unconfirm memory" }));
    expect(props.onConfirm).toHaveBeenCalledWith("memory-1", false);
    await user.click(screen.getByRole("button", { name: "Memory actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Delete memory" }));
    expect(props.onDelete).toHaveBeenCalledWith("memory-1");
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
  });

  it("keeps archived words and opacity plus a quiet pinned indicator", () => {
    render(<MemoryListRow memory={makeMemory({ is_archived: true, pinned: true })} style={{ opacity: 1 }} {...callbacks()} />);
    expect(screen.getByRole("article")).toHaveStyle({ opacity: "0.55", "--mem-enter-opacity": "0.55" });
    expect(screen.getByText("archived")).toBeVisible();
    expect(screen.getByText("Pinned")).toBeVisible();
  });

  it("keeps pending update content and accept visible without opening actions", async () => {
    vi.mocked(getPendingRevision).mockResolvedValueOnce({ source_id: "memory-1", content: "New captured preference.", source_agent: "codex" });
    const updated = vi.fn();
    window.addEventListener("memory-updated", updated);
    try {
      render(<MemoryListRow memory={makeMemory({ memory_type: "preference", confirmed: true })} {...callbacks()} />);
      expect(await screen.findByText("New captured preference.")).toBeVisible();
      expect(screen.getByText("Proposed update from ChatGPT / Codex")).toBeVisible();
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Accept update" }));
      await waitFor(() => expect(acceptPendingRevision).toHaveBeenCalledWith("memory-1"));
      expect(updated).toHaveBeenCalledOnce();
      expect(screen.queryByText("New captured preference.")).not.toBeInTheDocument();
    } finally {
      window.removeEventListener("memory-updated", updated);
    }
  });

  it("dismisses the visible pending update through the original API", async () => {
    vi.mocked(getPendingRevision).mockResolvedValueOnce({ source_id: "memory-1", content: "Proposed preference.", source_agent: null });
    render(<MemoryListRow memory={makeMemory({ memory_type: "preference", stability: "confirmed" })} {...callbacks()} />);
    await userEvent.click(await screen.findByRole("button", { name: "Dismiss update" }));
    await waitFor(() => expect(dismissPendingRevision).toHaveBeenCalledWith("memory-1"));
    expect(screen.queryByText("Proposed preference.")).not.toBeInTheDocument();
    expect(acceptPendingRevision).not.toHaveBeenCalled();
  });
});
