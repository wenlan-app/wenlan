// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render as renderInto, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { useEffect, useState } from "react";
import { WikiWorkspace } from "./WikiWorkspace";
import { WikiDirectory } from "./WikiDirectory";
import { useWikiFolderCreation } from "./WikiFolderCreationContext";
import { TeamAccessProvider, teamAccessFor } from "../../../web/teamAccess";

// Panels read through react-query, so every render needs a query client.
// Each render gets its own.
function render(ui: ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderInto(ui, { wrapper: ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider> });
}

vi.mock("./PageInventoryPanel", () => ({
  PageInventoryPanel: ({ onNoteCountChange }: { onNoteCountChange?: (count: number | null) => void }) => {
    useEffect(() => { onNoteCountChange?.(145); }, [onNoteCountChange]);
    return <div data-testid="wiki-directory" />;
  },
}));

vi.mock("./ArchivedNotesPanel", () => ({ ArchivedNotesPanel: ({ onOpenPage }: { onOpenPage: (page: { id: string }) => void }) => <div data-testid="archived-notes"><button type="button" onClick={() => onOpenPage({ id: "archived-one" })}>Open archived note</button></div> }));

describe("WikiWorkspace", () => {
  it("archive browse request opens the narrow drawer, reopens it and closes on selection", async () => {
    const onOpenPage = vi.fn();
    function BrowseArchive() {
      const [mode, setMode] = useState<"folders" | "archived">("folders");
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
      expect(screen.getByTestId("archived-notes")).toBeInTheDocument();
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
    rerender(<WikiWorkspace {...props} directoryOpenRequest={8} currentPageId="other"><textarea /></WikiWorkspace>);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  it("shows the folder tree with no browse-mode menu (UX-FIND-4)", () => {
    render(<WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}><textarea /></WikiWorkspace>);
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Folders" })).toBeInTheDocument();
    expect(screen.getByTestId("wiki-directory")).toBeInTheDocument();
  });

  it("offers one New button at the top of the list with New page and New folder, but not among archived pages", () => {
    const onCreatePage = vi.fn();
    const props = { onBrowse: vi.fn(), onCreatePage, onOpenDraft: vi.fn(), onOpenPage: vi.fn() };
    const { rerender } = render(<WikiWorkspace {...props}><textarea /></WikiWorkspace>);
    expect(screen.queryByRole("button", { name: "New page" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New folder" })).not.toBeInTheDocument();
    const create = screen.getByRole("button", { name: "New page or folder" });
    expect(create).toHaveAttribute("aria-haspopup", "menu");
    fireEvent.click(create);
    const menu = screen.getByRole("menu", { name: "New page or folder" });
    expect(Array.from(menu.querySelectorAll("[role='menuitem']")).map(item => item.textContent)).toEqual(["New page", "New folder"]);
    // No folder form is registered here, so New folder is offered but unavailable.
    expect(screen.getByRole("menuitem", { name: "New folder" })).toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(create).toHaveFocus();
    fireEvent.click(create);
    fireEvent.click(screen.getByRole("menuitem", { name: "New page" }));
    expect(onCreatePage).toHaveBeenLastCalledWith(undefined);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    rerender(<WikiWorkspace {...props} inventoryMode="archived"><textarea /></WikiWorkspace>);
    expect(screen.queryByRole("button", { name: "New page or folder" })).not.toBeInTheDocument();
  });

  it("a team member's New menu has New page but no New folder, since folders stay on the owner's computer", () => {
    const props = { onBrowse: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn() };
    const access = teamAccessFor({ member_id: "m", name: "Amy", spaces: [{ space: "Field Notes", access: "write" }] });
    render(<TeamAccessProvider value={access}><WikiWorkspace {...props}><textarea /></WikiWorkspace></TeamAccessProvider>);
    fireEvent.click(screen.getByRole("button", { name: "New page or folder" }));
    const menu = screen.getByRole("menu", { name: "New page or folder" });
    expect(Array.from(menu.querySelectorAll("[role='menuitem']")).map(item => item.textContent)).toEqual(["New page"]);
  });

  it("UX-SKILL-12: the browser build offers Find skills as the desktop app does; a team member is offered none", () => {
    const props = { onBrowse: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn(), onFindSkills: vi.fn() };
    const items = () => Array.from(screen.getByRole("menu", { name: "New page or folder" }).querySelectorAll("[role='menuitem']")).map(item => item.textContent);
    const member = teamAccessFor({ member_id: "m", name: "Amy", spaces: [{ space: "Field Notes", access: "write" }] });
    const app = render(<WikiDirectory {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "New page or folder" }));
    expect(items()).toContain("Find skills");
    app.unmount();
    vi.stubGlobal("__WENLAN_WEB__", true);
    try {
      // The person's own Wenlan in a browser: the daemon has the routes.
      const web = render(<WikiDirectory {...props} />);
      fireEvent.click(screen.getByRole("button", { name: "New page or folder" }));
      expect(items()).toContain("Find skills");
      web.unmount();
      // A team member, given the very same way to open it: the entry is not
      // offered, since their browser has no skills of its own to hold.
      render(<TeamAccessProvider value={member}><WikiDirectory {...props} /></TeamAccessProvider>);
      fireEvent.click(screen.getByRole("button", { name: "New page or folder" }));
      expect(items()).toEqual(["New page"]);
      expect(screen.queryByRole("menuitem", { name: "Find skills" })).not.toBeInTheDocument();
      expect(props.onFindSkills).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("shows the page count as a number beside the Folders title", () => {
    render(<WikiWorkspace onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}><textarea /></WikiWorkspace>);
    const count = screen.getByTitle("145 pages");
    expect(count).toHaveTextContent("145");
    expect(count.previousElementSibling).toBe(screen.getByRole("heading", { name: "Folders" }));
    expect(screen.getByText("145 pages")).toHaveClass("sr-only");
  });

  it("registers one stable reading root and a shared right-toolbar host", () => {
    const onSharedReadingHost = vi.fn();
    const onSharedToolsHost = vi.fn();
    const { container } = render(<WikiWorkspace onSharedReadingHost={onSharedReadingHost} onSharedToolsHost={onSharedToolsHost}
      onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}><textarea /></WikiWorkspace>);
    const reading = container.querySelector<HTMLElement>(".wiki-workspace-reading")!;
    const tools = container.querySelector<HTMLElement>(".wiki-shared-tools-rail .wiki-shared-document-tools")!;
    expect(onSharedReadingHost).toHaveBeenLastCalledWith(reading);
    expect(onSharedToolsHost).toHaveBeenLastCalledWith(tools);
    expect(reading).toContainElement(tools);
  });

  it("opens the mobile pages panel and the registered folder form", async () => {
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
    const onRevealRequest = vi.fn();
    render(
      <WikiDirectory onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onRevealRequest={onRevealRequest}>
        <RequestFolder />
      </WikiDirectory>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Request folder" }));
    // The directory asks its host to show it (the narrow drawer opens).
    await waitFor(() => expect(onRevealRequest).toHaveBeenCalledOnce());
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
      <WikiDirectory inventoryMode="folders" onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onCreationFolderPathChange={onCreationFolderPathChange}>
        <ReportPath />
      </WikiDirectory>,
    );

    await waitFor(() => expect(onCreationFolderPathChange).toHaveBeenLastCalledWith("Work/Research"));
  });

  it("reports no folder creation path while archived pages are shown", async () => {
    const onCreationFolderPathChange = vi.fn();
    function ReportPath() {
      const folderCreation = useWikiFolderCreation();
      useEffect(() => {
        folderCreation?.reportCreationFolderPath("Work/Research");
      }, [folderCreation?.reportCreationFolderPath]);
      return null;
    }
    render(
      <WikiDirectory inventoryMode="archived" onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()} onCreationFolderPathChange={onCreationFolderPathChange}>
        <ReportPath />
      </WikiDirectory>,
    );

    await waitFor(() => expect(onCreationFolderPathChange).toHaveBeenLastCalledWith(null));
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
    const props = { directoryHidden: true, onBrowse: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn() };
    const { container, rerender } = render(<WikiWorkspace {...props}><textarea data-testid="kept-editor" defaultValue="unsaved" /></WikiWorkspace>);
    const editor = screen.getByTestId("kept-editor");
    expect(container.querySelector(".wiki-workspace")).toHaveAttribute("data-directory-hidden", "true");
    rerender(<WikiWorkspace {...props} inventoryMode="archived"><textarea data-testid="kept-editor" defaultValue="unsaved" /></WikiWorkspace>);
    expect(screen.getByTestId("archived-notes")).toBeInTheDocument();
    expect(container.querySelector(".wiki-workspace")).not.toHaveAttribute("data-directory-hidden");
    expect(screen.getByTestId("kept-editor")).toBe(editor);
    expect(editor).toHaveValue("unsaved");
  });

  it("goes back from Archived pages to the folder tree", () => {
    function Archive() {
      const [mode, setMode] = useState<"folders" | "archived">("archived");
      return <WikiWorkspace inventoryMode={mode} onInventoryModeChange={setMode} onBrowse={vi.fn()} onCreatePage={vi.fn()} onOpenDraft={vi.fn()} onOpenPage={vi.fn()}><textarea /></WikiWorkspace>;
    }
    render(<Archive />);
    expect(screen.getByTestId("archived-notes")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back to folders" }));
    expect(screen.queryByTestId("archived-notes")).not.toBeInTheDocument();
    expect(screen.getByTestId("wiki-directory")).toBeInTheDocument();
  });

});
