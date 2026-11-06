// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "../PageDetail";
import * as tauri from "../../../lib/tauri";
import * as pageExportApi from "../../../lib/pageExportApi";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";

vi.mock("../../../lib/tauri");
vi.mock("../../../lib/pageExportApi");

const MOCK_PAGE: tauri.Page = {
  id: "c1",
  title: "Test Page",
  content: "Some page content",
  summary: "A test summary",
  domain: "testing",
  entity_id: null,
  version: 1,
  status: "active",
  created_at: new Date().toISOString(),
  last_compiled: new Date().toISOString(),
  last_modified: new Date().toISOString(),
  source_memory_ids: [],
};

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

describe("PageDetail export", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(tauri.getPage).mockResolvedValue(MOCK_PAGE);
    vi.mocked(tauri.getPageLinks).mockResolvedValue({ outbound: [], inbound: [] });
    vi.mocked(tauri.getPageRevisions).mockResolvedValue({
      page_id: "c1",
      current_version: 1,
      user_edited: false,
      entries: [],
    });
    vi.mocked(tauri.getPageSources).mockResolvedValue([]);
    vi.mocked(tauri.listOrphanLinks).mockResolvedValue({ min_count: 2, orphan_labels: [] });
    vi.mocked(tauri.listPages).mockResolvedValue([MOCK_PAGE]);
  });

  function sources(...names: string[]): tauri.RegisteredSource[] {
    return names.map((name, index) => ({
      id: `obsidian-vault-${index}`, source_type: "obsidian", path: `/Users/test/${name}`,
      status: "Active" as const, last_sync: null, file_count: 10, memory_count: 20,
    }));
  }

  async function renderMenu() {
    const user = userEvent.setup();
    render(<PageDetail pageId="c1" onBack={vi.fn()} onMemoryClick={vi.fn()} />, { wrapper });
    const trigger = await screen.findByRole("button", { name: "Page actions" });
    await user.click(trigger);
    return { user, trigger, menu: screen.getByRole("menu", { name: "Page actions" }) };
  }

  it("offers no per-page export even with an Obsidian vault connected", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue(sources("vault-one", "vault-two"));
    await renderMenu();
    expect(screen.queryByRole("menuitem", { name: /^Export to/ })).toBeNull();
    expect(tauri.exportPageToObsidian).not.toHaveBeenCalled();
  });

  it("captures and exports the freshly saved version when opened", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue(sources("vault-one"));
    const bundle: pageExportApi.PageExportBundle = { id: "saved-v2", digest: "digest-v2", kind: "saved", page_id: "c1", page_incarnation: "inc", space_id: null, version: 2, export_revision: 2, source_revision: 1, title: "Fresh title", content: "Fresh saved content", last_modified: "2026-11-03T12:00:00Z", captured_at: 1793707200000, citations: [], sources: [], bibliography: [], references_available: true, pending_revision_id: null };
    vi.mocked(pageExportApi.capturePageExport).mockResolvedValue(bundle);
    vi.mocked(pageExportApi.listPageExportVersions).mockResolvedValue({ versions: [{ bundle_id: bundle.id, kind: "saved", version: 2, export_revision: 2, title: bundle.title, saved_at: bundle.last_modified, captured_at: bundle.captured_at, references_available: true, bibliography_available: true, unavailable_reason: null }] });
    vi.mocked(pageExportApi.getPageExportBundle).mockResolvedValue(bundle);
    vi.mocked(save).mockResolvedValue("/tmp/fresh.md");
    const { user } = await renderMenu();
    const action = screen.getByRole("menuitem", { name: "Export…" });
    expect(action).toBeEnabled();
    await user.click(action);
    expect(await screen.findByRole("dialog", { name: "Export page" })).toBeInTheDocument();
    await waitFor(() => expect(pageExportApi.capturePageExport).toHaveBeenCalledWith("c1"));
    expect(await screen.findByLabelText("Version")).toHaveValue("saved-v2");
    await waitFor(() => expect(screen.getByRole("button", { name: "Export", exact: true })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Export", exact: true }));
    await waitFor(() => expect(writeTextFile).toHaveBeenCalledOnce());
    expect(vi.mocked(writeTextFile).mock.calls[0][1]).toContain("Fresh saved content");
    expect(pageExportApi.getPageExportBundle).toHaveBeenCalledTimes(2);
    expect(pageExportApi.getPageExportBundle).toHaveBeenLastCalledWith("c1", "saved-v2");
  });


  it("closes the page menu with Escape and restores its stable title trigger", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue(sources("vault-one", "vault-two"));
    const { trigger, menu } = await renderMenu();
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("navigates all enabled commands and keeps Escape out of Main history", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue(sources("vault-one", "vault-two"));
    const observer = vi.fn();
    window.addEventListener("keydown", observer);
    try {
      const { user, menu, trigger } = await renderMenu();
      const items = Array.from(menu.querySelectorAll<HTMLElement>('[role^="menuitem"]'))
        .filter((item) => !item.hasAttribute("disabled"));
      expect(items[0]).toHaveFocus();
      for (let index = 1; index < items.length; index++) {
        await user.keyboard("{ArrowDown}");
        expect(items[index]).toHaveFocus();
      }
      await user.keyboard("{ArrowDown}");
      expect(items[0]).toHaveFocus();
      await user.keyboard("{ArrowUp}");
      expect(items[items.length - 1]).toHaveFocus();
      await user.keyboard("{Home}");
      expect(items[0]).toHaveFocus();
      await user.keyboard("{End}");
      expect(items[items.length - 1]).toHaveFocus();
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
      expect(trigger).toHaveFocus();
      expect(observer).not.toHaveBeenCalled();
    } finally { window.removeEventListener("keydown", observer); }
  });

  it("opens the title menu at either enabled boundary with arrow keys", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue(sources("vault-one", "vault-two"));
    const { user, trigger } = await renderMenu();
    await user.keyboard("{Escape}");
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    const firstItems = screen.getAllByRole("menuitem").filter((item) => !item.hasAttribute("disabled"));
    expect(firstItems[0]).toHaveFocus();
    await user.keyboard("{Escape}");
    await user.keyboard("{ArrowUp}");
    const lastItems = screen.getAllByRole("menuitem").filter((item) => !item.hasAttribute("disabled"));
    expect(lastItems[lastItems.length - 1]).toHaveFocus();
  });

});
