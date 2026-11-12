// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import type { Space } from "../../../lib/tauri";
import { SpaceDossierHeader } from "./SpaceDossierHeader";
import { SPACE_DETAIL_TEST_COPY } from "./testTranslation";

const space: Space = {
  id: "space-1",
  name: "Wenlan",
  description: "Editorial memory",
  suggested: false,
  starred: false,
  sort_order: 0,
  memory_count: 12,
  entity_count: 3,
  created_at: 1_000,
  updated_at: 2_000,
};

function renderHeader(mutationError = false, onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void) {
  const actions = {
    onBack: vi.fn(),
    onCreatePage: vi.fn(),
    onDelete: vi.fn(),
    onKeep: vi.fn(),
    onSaveIdentity: vi.fn<(value: { name: string; description: string }) => Promise<void> | void>(),
  };
  const result = render(
    <SpaceDossierHeader
      actions={actions}
      copy={SPACE_DETAIL_TEST_COPY}
      mutationError={mutationError}
      space={space}
      onRegisterFlush={onRegisterFlush}
    />,
  );
  return {
    ...result,
    actions,
    rerenderHeader: (nextMutationError: boolean) => result.rerender(
      <SpaceDossierHeader
        actions={actions}
        copy={SPACE_DETAIL_TEST_COPY}
        mutationError={nextMutationError}
        space={space}
        onRegisterFlush={onRegisterFlush}
      />,
    ),
  };
}

describe("SpaceDossierHeader identity editing", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("exposes the title and description directly without an edit mode", () => {
    renderHeader();
    expect(screen.getByRole("heading", { level: 1, name: "Wenlan" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Wenlan");
    expect(screen.getByRole("textbox", { name: "Edit description" })).toHaveValue("Editorial memory");
    expect(screen.queryByRole("button", { name: "Edit space" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("saves the combined draft after typing pauses and ignores unchanged blur", async () => {
    vi.useFakeTimers();
    try {
      const { actions } = renderHeader();
      const input = screen.getByRole("textbox", { name: "Name" });
      fireEvent.blur(input);
      expect(actions.onSaveIdentity).not.toHaveBeenCalled();
      fireEvent.change(input, { target: { value: " Wenlan research " } });
      fireEvent.change(screen.getByRole("textbox", { name: "Edit description" }), {
        target: { value: " A retained description " },
      });
      await act(() => vi.advanceTimersByTimeAsync(749));
      expect(actions.onSaveIdentity).not.toHaveBeenCalled();
      await act(() => vi.advanceTimersByTimeAsync(1));
      expect(actions.onSaveIdentity).toHaveBeenCalledExactlyOnceWith({ name: "Wenlan research", description: "A retained description" });
      expect(input).toHaveValue("Wenlan research");
    } finally { vi.useRealTimers(); }
  });

  it("contains Escape before history navigation and flushes without discarding", async () => {
    const { actions } = renderHeader();
    const navigateBack = () => actions.onBack();
    window.addEventListener("keydown", navigateBack);
    const input = screen.getByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: "Wenlan research" } });
    const accepted = fireEvent.keyDown(input, { key: "Escape" });
    window.removeEventListener("keydown", navigateBack);
    expect(accepted).toBe(false);
    expect(actions.onBack).not.toHaveBeenCalled();
    await waitFor(() => expect(actions.onSaveIdentity).toHaveBeenCalledExactlyOnceWith({ name: "Wenlan research", description: "Editorial memory" }));
    expect(input).toHaveValue("Wenlan research");
  });

  it("preserves a failed draft and retries explicitly from the same surface", async () => {
    const { actions } = renderHeader();
    actions.onSaveIdentity.mockRejectedValueOnce(new Error("Unavailable"));
    const input = screen.getByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: " Wenlan research " } });
    fireEvent.blur(input);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save this change.");
    expect(input).toHaveValue(" Wenlan research ");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(actions.onSaveIdentity).toHaveBeenCalledTimes(2);
    expect(input).toHaveValue("Wenlan research");
  });

  it("serializes saves and preserves text entered during an in-flight write", async () => {
    const { actions } = renderHeader();
    let finish!: () => void;
    actions.onSaveIdentity.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const input = screen.getByRole("textbox", { name: "Name" });
    const description = screen.getByRole("textbox", { name: "Edit description" });
    fireEvent.change(input, { target: { value: "Wenlan research" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.change(description, { target: { value: "Newer typing" } });
    fireEvent.blur(description);
    expect(actions.onSaveIdentity).toHaveBeenCalledTimes(1);
    await act(async () => finish());
    await waitFor(() => expect(actions.onSaveIdentity).toHaveBeenCalledTimes(2));
    expect(actions.onSaveIdentity.mock.calls[1][0]).toEqual({ name: "Wenlan research", description: "Newer typing" });
    expect(description).toHaveValue("Newer typing");
  });

  it("does not submit an empty name or an IME confirmation key", async () => {
    const { actions } = renderHeader();
    const input = screen.getByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: "新的名稱" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(actions.onSaveIdentity).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: " " } });
    fireEvent.blur(input);
    expect(actions.onSaveIdentity).not.toHaveBeenCalled();
  });

  it("keeps trailing spaces and the same input while an automatic save lands", async () => {
    vi.useFakeTimers();
    try {
      const { actions } = renderHeader();
      const input = screen.getByRole("textbox", { name: "Name" });
      fireEvent.focus(input);
      fireEvent.change(input, { target: { value: "Reading " } });
      await act(() => vi.advanceTimersByTimeAsync(750));
      expect(actions.onSaveIdentity).toHaveBeenCalledWith({ name: "Reading", description: "Editorial memory" });
      expect(input).toHaveValue("Reading ");
      fireEvent.change(input, { target: { value: "Reading club" } });
      await act(() => vi.advanceTimersByTimeAsync(750));
      expect(actions.onSaveIdentity).toHaveBeenLastCalledWith({ name: "Reading club", description: "Editorial memory" });
    } finally { vi.useRealTimers(); }
  });

  it("waits until IME composition ends before autosaving", async () => {
    vi.useFakeTimers();
    try {
      const { actions } = renderHeader();
      const input = screen.getByRole("textbox", { name: "Name" });
      fireEvent.compositionStart(input);
      fireEvent.change(input, { target: { value: "閱讀計畫" } });
      await act(() => vi.advanceTimersByTimeAsync(1_000));
      expect(actions.onSaveIdentity).not.toHaveBeenCalled();
      fireEvent.compositionEnd(input);
      await act(() => vi.advanceTimersByTimeAsync(750));
      expect(actions.onSaveIdentity).toHaveBeenCalledExactlyOnceWith({ name: "閱讀計畫", description: "Editorial memory" });
    } finally { vi.useRealTimers(); }
  });

  it("keeps a dirty draft across polling and unregisters without saving after unmount", async () => {
    const onRegisterFlush = vi.fn();
    const { actions, rerenderHeader, unmount } = renderHeader(false, onRegisterFlush);
    fireEvent.change(screen.getByRole("textbox", { name: "Edit description" }), { target: { value: "Draft" } });
    rerenderHeader(false);
    expect(screen.getByRole("textbox", { name: "Edit description" })).toHaveValue("Draft");
    unmount();
    expect(actions.onSaveIdentity).not.toHaveBeenCalled();
    expect(onRegisterFlush).toHaveBeenLastCalledWith(null);
  });

  it("flushes a pending identity edit and any newer typing before resolving", async () => {
    let flush: (() => Promise<boolean>) | null = null;
    const { actions } = renderHeader(false, (next) => { flush = next; });
    let finishFirst!: () => void;
    actions.onSaveIdentity.mockImplementationOnce(() => new Promise<void>((resolve) => { finishFirst = resolve; }));
    const description = screen.getByRole("textbox", { name: "Edit description" });
    fireEvent.change(description, { target: { value: "First draft" } });
    const pending = flush!();
    await waitFor(() => expect(actions.onSaveIdentity).toHaveBeenCalledTimes(1));
    fireEvent.change(description, { target: { value: "Newer draft" } });
    await act(async () => finishFirst());
    await expect(pending).resolves.toBe(true);
    expect(actions.onSaveIdentity).toHaveBeenCalledTimes(2);
    expect(actions.onSaveIdentity.mock.calls[1]?.[0]).toEqual({ name: "Wenlan", description: "Newer draft" });
    expect(description).toHaveValue("Newer draft");
  });

  it("returns false on a failed flush while keeping the draft editable", async () => {
    let flush: (() => Promise<boolean>) | null = null;
    const { actions } = renderHeader(false, (next) => { flush = next; });
    actions.onSaveIdentity.mockRejectedValueOnce(new Error("offline"));
    const description = screen.getByRole("textbox", { name: "Edit description" });
    fireEvent.change(description, { target: { value: "Keep this draft" } });
    let saved = true;
    await act(async () => { saved = await flush!(); });
    expect(saved).toBe(false);
    expect(description).toHaveValue("Keep this draft");
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save this change.");
  });

  it("keeps Delete inside the overflow menu", async () => {
    // Given a confirmed Space header
    const user = userEvent.setup();
    renderHeader();

    // Then Delete is unavailable until overflow is opened
    expect(screen.queryByRole("menuitem", { name: "Delete space" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Actions for Wenlan" }));
    expect(screen.getByRole("menuitem", { name: "Delete space" })).toBeInTheDocument();

    // When Delete is chosen, the existing explicit confirmation still gates it
    await user.click(screen.getByRole("menuitem", { name: "Delete space" }));
    expect(screen.getByRole("button", { name: "Confirm delete space" })).toBeInTheDocument();
  });

  it("offers Show on the big map in the overflow menu and hands this Space off", async () => {
    const user = userEvent.setup();
    const onOpenGraph = vi.fn();
    render(
      <SpaceDossierHeader
        actions={{ onBack: vi.fn(), onCreatePage: vi.fn(), onDelete: vi.fn(), onKeep: vi.fn(), onSaveIdentity: vi.fn(), onOpenGraph }}
        copy={SPACE_DETAIL_TEST_COPY}
        mutationError={false}
        space={space}
      />,
    );
    await user.click(screen.getByRole("button", { name: "Actions for Wenlan" }));
    await user.click(screen.getByRole("menuitem", { name: "Show on the big map" }));
    expect(onOpenGraph).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("leaves Show on the big map out when there is nowhere to hand off", async () => {
    const user = userEvent.setup();
    renderHeader();
    await user.click(screen.getByRole("button", { name: "Actions for Wenlan" }));
    expect(screen.queryByRole("menuitem", { name: "Show on the big map" })).not.toBeInTheDocument();
  });

  it("closes the overflow when clicking outside", async () => {
    const user = userEvent.setup();
    renderHeader();
    await user.click(screen.getByRole("button", { name: "Actions for Wenlan" }));
    await user.click(screen.getByRole("heading", { level: 1 }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("moves focus into the overflow menu and contains its navigation keys", async () => {
    // Given the overflow trigger
    const user = userEvent.setup();
    renderHeader();
    await user.click(screen.getByRole("button", { name: "Actions for Wenlan" }));
    const menu = screen.getByRole("menu");
    const deleteItem = screen.getByRole("menuitem", { name: "Delete space" });

    // Then opening follows the menu focus contract
    expect(deleteItem).toHaveFocus();

    // And the menu owns the standard composite navigation keys
    for (const key of ["ArrowDown", "ArrowUp", "Home", "End"]) {
      expect(fireEvent.keyDown(menu, { key })).toBe(false);
      expect(deleteItem).toHaveFocus();
    }
  });

  it("opens the overflow menu from its trigger with ArrowDown or ArrowUp", async () => {
    const user = userEvent.setup();
    renderHeader();
    const trigger = screen.getByRole("button", { name: "Actions for Wenlan" });

    trigger.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Delete space" })).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(trigger).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("menuitem", { name: "Delete space" })).toHaveFocus();
  });

  it("contains menu Escape before Main history navigation and restores trigger focus", async () => {
    // Given the menu is open above Main's window-level Escape navigation
    const user = userEvent.setup();
    const { actions } = renderHeader();
    const navigateBack = () => actions.onBack();
    window.addEventListener("keydown", navigateBack);
    const trigger = screen.getByRole("button", { name: "Actions for Wenlan" });
    await user.click(trigger);

    // When Escape is dispatched from the focused menu item
    const accepted = fireEvent.keyDown(screen.getByRole("menuitem", { name: "Delete space" }), {
      key: "Escape",
    });
    window.removeEventListener("keydown", navigateBack);

    // Then only the menu closes, without changing Space history
    expect(accepted).toBe(false);
    expect(actions.onBack).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
});
