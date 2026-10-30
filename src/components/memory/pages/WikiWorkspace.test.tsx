// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useEffect, useState } from "react";
import { RECENT_PAGES_STORAGE_KEY } from "../../../lib/recentPages";
import { WikiWorkspace } from "./WikiWorkspace";
import { useWikiFolderCreation } from "./WikiFolderCreationContext";

vi.mock("./PageInventoryPanel", () => ({
  PageInventoryPanel: ({ onNoteCountChange }: { onNoteCountChange?: (count: number | null) => void }) => {
    useEffect(() => { onNoteCountChange?.(145); }, [onNoteCountChange]);
    return <div data-testid="wiki-directory" />;
  },
}));

vi.mock("./ArchivedNotesPanel", () => ({ ArchivedNotesPanel: ({ onOpenPage }: { onOpenPage: (page: { id: string }) => void }) => <div data-testid="archived-notes"><button type="button" onClick={() => onOpenPage({ id: "archived-one" })}>Open archived note</button></div> }));

describe("WikiWorkspace", () => {
  beforeEach(() => localStorage.removeItem("wenlan:wiki-inventory:v1"));
  it.each(["recent", "archived"] as const)("archive browse request opens the narrow drawer from %s, reopens it and closes on selection", async initialMode => {
    const onOpenPage = vi.fn();
    function BrowseArchive() {
      const [mode, setMode] = useState<"recent" | "archived">(initialMode);
      const [request, setRequest] = useState(0);
      return <WikiWorkspace directoryHidden inventoryMode={mode} directoryOpenRequest={request}
        onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={onOpenPage}>
        <button type="button" onClick={() => { setMode("archived"); setRequest(value => value + 1); }}>Browse archived notes</button>
        <textarea data-testid="retained-editor" defaultValue="unsaved text" />
      </WikiWorkspace>;
    }
    const { container } = render(<BrowseArchive />);
    const editor = screen.getByTestId("retained-editor");
    const drawer = container.querySelector(".wiki-workspace-body");
    const toggle = screen.getByRole("button", { name: "Pages" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    for (let attempt = 0; attempt < 2; attempt += 1) {
      fireEvent.click(screen.getByRole("button", { name: "Browse archived notes" }));
      await waitFor(() => expect(toggle).toHaveAttribute("aria-expanded", "true"));
      // This is the existing container-query CSS selector that reveals the narrow drawer.
      expect(drawer).toHaveClass("is-folder-open");
      expect(container.querySelector(".wiki-workspace")).not.toHaveAttribute("data-directory-hidden");
      expect(screen.getByRole("combobox")).toHaveValue("archived");
      expect(screen.getByTestId("retained-editor")).toBe(editor);
      expect(editor).toHaveValue("unsaved text");
      fireEvent.click(screen.getByRole("button", { name: "Open archived note" }));
      expect(onOpenPage).toHaveBeenLastCalledWith({ id: "archived-one" });
      expect(toggle).toHaveAttribute("aria-expanded", "false");
      expect(drawer).not.toHaveClass("is-folder-open");
    }
  });

  it("does not replay an old directory request on mount or unrelated rerenders", () => {
    const props = { inventoryMode: "archived" as const, onBrowse: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn() };
    const { rerender } = render(<WikiWorkspace {...props} directoryOpenRequest={7}><textarea /></WikiWorkspace>);
    const toggle = screen.getByRole("button", { name: "Pages" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    rerender(<WikiWorkspace {...props} directoryOpenRequest={8}><textarea /></WikiWorkspace>);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(toggle);
    rerender(<WikiWorkspace {...props} directoryOpenRequest={8} recentRevision={1}><textarea /></WikiWorkspace>);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  it("defaults to recent pages and switches modes without remounting the active editor", () => {
    const { getByTestId } = render(
      <WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}>
        <textarea data-testid="active-editor" defaultValue="unsaved text" />
      </WikiWorkspace>,
    );
    const editor = getByTestId("active-editor");
    const mode = screen.getByRole("combobox");

    expect(mode).toHaveValue("recent");
    expect(screen.queryByTestId("wiki-directory")).not.toBeInTheDocument();
    fireEvent.change(mode, { target: { value: "folders" } });
    expect(screen.getByTestId("wiki-directory")).toBeInTheDocument();
    expect(getByTestId("active-editor")).toBe(editor);
    fireEvent.change(mode, { target: { value: "custom" } });
    expect(screen.queryByTestId("wiki-directory")).not.toBeInTheDocument();
    expect(getByTestId("active-editor")).toBe(editor);
    fireEvent.change(mode, { target: { value: "recent" } });
    expect(screen.queryByTestId("wiki-directory")).not.toBeInTheDocument();
    expect(getByTestId("active-editor")).toBe(editor);
  });

  it("shows the Folders page count as a number beside the title, and only in folders mode", () => {
    render(<WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}><textarea /></WikiWorkspace>);
    const mode = screen.getByRole("combobox");
    expect(screen.queryByText("145 pages")).not.toBeInTheDocument();
    fireEvent.change(mode, { target: { value: "folders" } });
    const count = screen.getByTitle("145 pages");
    expect(count).toHaveTextContent("145");
    expect(count.previousElementSibling).toBe(mode.closest("label"));
    expect(screen.getByText("145 pages")).toHaveClass("sr-only");
    fireEvent.change(mode, { target: { value: "recent" } });
    expect(screen.queryByTitle("145 pages")).not.toBeInTheDocument();
  });

  it("restores the chosen custom mode after remount without entering the folder tree", () => {
    const props = { onBrowse: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn() };
    const first = render(<WikiWorkspace {...props}><textarea /></WikiWorkspace>);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "custom" } });
    first.unmount();
    render(<WikiWorkspace {...props}><textarea /></WikiWorkspace>);
    expect(screen.getByRole("combobox")).toHaveValue("custom");
    expect(screen.queryByTestId("wiki-directory")).not.toBeInTheDocument();
  });

  it("opens the mobile pages panel and registered folder form only in folders mode", async () => {
    const openForm = vi.fn();
    function RequestFolder() {
      const folderCreation = useWikiFolderCreation();
      useEffect(() => {
        if (!folderCreation) return;
        folderCreation.reportCanCreateFolder(true);
        return folderCreation.registerCreateFolderForm(openForm);
      }, [folderCreation?.reportCanCreateFolder, folderCreation?.registerCreateFolderForm]);
      return <button type="button" onClick={() => folderCreation?.requestCreateFolder()}>Request folder</button>;
    }
    render(
      <WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}>
        <RequestFolder />
      </WikiWorkspace>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Request folder" }));
    expect(openForm).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "folders" } });
    fireEvent.click(screen.getByRole("button", { name: "Request folder" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Pages" })).toHaveAttribute("aria-expanded", "true"));
    expect(openForm).toHaveBeenCalledOnce();
  });

  it("reports creation folder changes to its parent", async () => {
    const onCreationFolderPathChange = vi.fn();
    function ReportPath() {
      const folderCreation = useWikiFolderCreation();
      useEffect(() => {
        folderCreation?.reportCreationFolderPath("Work/Research");
      }, [folderCreation?.reportCreationFolderPath]);
      return null;
    }
    render(
      <WikiWorkspace inventoryMode="folders" onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onCreationFolderPathChange={onCreationFolderPathChange}>
        <ReportPath />
      </WikiWorkspace>,
    );

    await waitFor(() => expect(onCreationFolderPathChange).toHaveBeenLastCalledWith("Work/Research"));
  });

  it("reports no folder creation path while recent pages are selected", async () => {
    const onCreationFolderPathChange = vi.fn();
    function ReportPath() {
      const folderCreation = useWikiFolderCreation();
      useEffect(() => {
        folderCreation?.reportCreationFolderPath("Work/Research");
      }, [folderCreation?.reportCreationFolderPath]);
      return null;
    }
    render(
      <WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onCreationFolderPathChange={onCreationFolderPathChange}>
        <ReportPath />
      </WikiWorkspace>,
    );

    await waitFor(() => expect(onCreationFolderPathChange).toHaveBeenLastCalledWith(null));
  });

  it("renders the persisted recent history in visit order and marks the active page", () => {
    localStorage.setItem(RECENT_PAGES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [
        { id: "newer", title: "Newer page", visitedAt: Date.now() },
        { id: "older", title: "Older page", visitedAt: Date.now() - 10 },
      ],
    }));
    const onOpenRecentPage = vi.fn();
    render(
      <WikiWorkspace currentPageId="older" onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onOpenRecentPage={onOpenRecentPage}>
        <textarea data-testid="active-editor" />
      </WikiWorkspace>,
    );

    const newer = screen.getByRole("button", { name: "Newer page" });
    const older = screen.getByRole("button", { name: "Older page" });
    expect(newer.compareDocumentPosition(older) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(older).toHaveAttribute("aria-current", "page");
    fireEvent.click(newer);
    expect(onOpenRecentPage).toHaveBeenCalledWith("newer");
  });

  it("closes the pages drawer after opening a recent page", () => {
    localStorage.setItem(RECENT_PAGES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [{ id: "recent-page", title: "Recent page", visitedAt: Date.now() }],
    }));
    const onOpenRecentPage = vi.fn();
    render(
      <WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onOpenRecentPage={onOpenRecentPage}>
        <textarea data-testid="active-editor" />
      </WikiWorkspace>,
    );
    const notesToggle = screen.getByRole("button", { name: "Pages" });

    fireEvent.click(notesToggle);
    expect(notesToggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Recent page" }));
    expect(onOpenRecentPage).toHaveBeenCalledWith("recent-page");
    expect(notesToggle).toHaveAttribute("aria-expanded", "false");
  });

  it("shows the empty recent-notes state without inventing entries", () => {
    localStorage.removeItem(RECENT_PAGES_STORAGE_KEY);
    const { container } = render(
      <WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}>
        <textarea data-testid="active-editor" />
      </WikiWorkspace>,
    );

    expect(container.querySelector(".wiki-recent-notes-empty")).toBeInTheDocument();
    expect(container.querySelector(".wiki-recent-notes-list")).not.toBeInTheDocument();
  });

  it("marks the workspace only while the directory is hidden so CSS can drop it", () => {
    const props = { onBrowse: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn() };
    const { container, rerender } = render(<WikiWorkspace {...props}><textarea /></WikiWorkspace>);
    const workspace = container.querySelector(".wiki-workspace");
    expect(workspace).not.toHaveAttribute("data-directory-hidden");

    rerender(<WikiWorkspace {...props} directoryHidden><textarea /></WikiWorkspace>);
    expect(workspace).toHaveAttribute("data-directory-hidden", "true");
    // The folders toggle and directory stay mounted (CSS hides them) so the
    // two-column layout returns without remounting the reading column.
    expect(container.querySelector(".wiki-workspace-directory")).toBeInTheDocument();

    rerender(<WikiWorkspace {...props} directoryHidden={false}><textarea /></WikiWorkspace>);
    expect(workspace).not.toHaveAttribute("data-directory-hidden");
  });
  it("shows the archive directory in an otherwise empty workspace without replacing the editor", () => {
    const { container } = render(<WikiWorkspace directoryHidden onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}><textarea data-testid="kept-editor" defaultValue="unsaved" /></WikiWorkspace>);
    const editor = screen.getByTestId("kept-editor");
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "archived" } });
    expect(screen.getByTestId("archived-notes")).toBeInTheDocument();
    expect(container.querySelector(".wiki-workspace")).not.toHaveAttribute("data-directory-hidden");
    expect(screen.getByTestId("kept-editor")).toBe(editor);
    expect(editor).toHaveValue("unsaved");
    expect(JSON.parse(localStorage.getItem("wenlan:wiki-inventory:v1")!).mode).toBe("archived");
  });

});
