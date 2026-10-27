// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  getBackgroundAiEnabled,
  getResolvedRouting,
  listAgents,
  listNewMaterial,
  listRegisteredSources,
  type RegisteredSource,
} from "../../../lib/tauri";
import { OrganizeActionsContext, type OrganizeActions } from "../askAi/TurnIntoPages";
import { FolderImportOrganize, forgetAddedFolder, rememberAddedFolder } from "./FolderImportOrganize";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getBackgroundAiEnabled: vi.fn(),
  getResolvedRouting: vi.fn(async () => null),
  getOnDeviceModel: vi.fn(),
  listAgents: vi.fn(async () => []),
  listNewMaterial: vi.fn(),
  listRegisteredSources: vi.fn(),
  listPagesExplicitBrowse: vi.fn(async () => []),
}));

const NOW = Math.floor(Date.now() / 1000);

function source(overrides: Partial<RegisteredSource> = {}): RegisteredSource {
  return {
    id: "vault",
    source_type: "obsidian",
    path: "/Users/me/Notes",
    status: "Active",
    last_sync: NOW,
    file_count: 12,
    memory_count: 31,
    ...overrides,
  };
}

function codex() {
  return {
    id: "agent-codex", name: "codex", display_name: "Codex", agent_type: "codex", description: null, enabled: true,
    trust_level: "full", last_seen_at: NOW, memory_count: 4, created_at: NOW - 3600, updated_at: NOW,
  };
}

const actions = (): OrganizeActions => ({
  onConnectTool: vi.fn(), onOpenIntelligence: vi.fn(), onOpenActivity: vi.fn(), onSelectPage: vi.fn(),
});

function wrap(children: ReactNode, value: OrganizeActions | null = actions()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <OrganizeActionsContext.Provider value={value}>{children}</OrganizeActionsContext.Provider>
    </QueryClientProvider>
  );
}

describe("FolderImportOrganize", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.clearAllMocks();
    forgetAddedFolder();
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(false);
    vi.mocked(getResolvedRouting).mockResolvedValue(null);
    vi.mocked(listAgents).mockResolvedValue([codex()] as never);
    vi.mocked(listRegisteredSources).mockResolvedValue([source()]);
    vi.mocked(listNewMaterial).mockResolvedValue({ items: [], total: 12, kinds: { note: 12 } });
  });
  afterEach(() => forgetAddedFolder());

  it("shows nothing when no folder was added", async () => {
    render(wrap(<FolderImportOrganize />));
    await act(async () => {});
    expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument();
    expect(listRegisteredSources).not.toHaveBeenCalled();
  });

  it("after a vault is added and read, offers to turn its notes into pages", async () => {
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />));
    await waitFor(() => {
      expect(screen.getByTestId("import-lead")).toHaveTextContent("12 notes imported. You can search them now.");
    });
    await waitFor(() => expect(screen.getByTestId("turn-into-pages")).toHaveAttribute("data-state", "ready"));
    expect(screen.getByRole("button", { name: "Write pages with Codex" })).toBeInTheDocument();
  });

  it("waits for the first read before it shows", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([source({ last_sync: null })]);
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />));
    await waitFor(() => expect(listRegisteredSources).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument();
    expect(listNewMaterial).not.toHaveBeenCalled();
  });

  it("counts a folder's stored files, not the files still being prepared", async () => {
    // 20 files handed to document prep; 7 stored so far.
    vi.mocked(listRegisteredSources).mockResolvedValue([source({ id: "docs", source_type: "directory", file_count: 20, memory_count: 20 })]);
    vi.mocked(listNewMaterial).mockResolvedValue({ items: [], total: 9, kinds: { file: 7, capture: 2 } });
    rememberAddedFolder({ id: "docs", source_type: "directory" });
    render(wrap(<FolderImportOrganize />));
    await waitFor(() => {
      expect(screen.getByTestId("import-lead")).toHaveTextContent("7 files imported. You can search them now.");
    });
  });

  it("never counts more than the folder holds", async () => {
    vi.mocked(listNewMaterial).mockResolvedValue({ items: [], total: 40, kinds: { note: 40 } });
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />));
    await waitFor(() => expect(screen.getByTestId("import-lead")).toHaveTextContent("12 notes imported."));
  });

  it("shows nothing while nothing from the folder is stored yet", async () => {
    vi.mocked(listNewMaterial).mockResolvedValue({ items: [], total: 0, kinds: {} });
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />));
    await waitFor(() => expect(listNewMaterial).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument();
  });

  it("inside a folder, shows only that folder's finish", async () => {
    rememberAddedFolder(source());
    render(wrap(<><FolderImportOrganize sourceId="other" /></>));
    await act(async () => {});
    expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument();
  });

  it("is put away with Later", async () => {
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />));
    fireEvent.click(await screen.findByRole("button", { name: "Later" }));
    await waitFor(() => expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument());
  });

  it("in Automatic, says who writes the pages instead of offering a button", async () => {
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(true);
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />));
    await waitFor(() => expect(screen.getByTestId("turn-pages-automatic")).toBeInTheDocument());
    expect(screen.getByTestId("import-lead")).toHaveTextContent("12 notes imported.");
    expect(screen.queryByTestId("turn-into-pages")).not.toBeInTheDocument();
  });

  it("outside the main window, where the card leads nowhere, shows nothing", async () => {
    rememberAddedFolder(source());
    render(wrap(<FolderImportOrganize />, null));
    await act(async () => {});
    expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument();
  });

  it("forgets a folder removed before it finished", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([]);
    rememberAddedFolder(source());
    const { rerender } = render(wrap(<FolderImportOrganize />));
    await waitFor(() => expect(listRegisteredSources).toHaveBeenCalled());
    await act(async () => {});
    vi.mocked(listRegisteredSources).mockResolvedValue([source()]);
    rerender(wrap(<FolderImportOrganize />));
    await act(async () => {});
    expect(screen.queryByTestId("folder-import-organize")).not.toBeInTheDocument();
  });
});
