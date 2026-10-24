// SPDX-License-Identifier: AGPL-3.0-only
import { invoke } from "@tauri-apps/api/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import TrayPanel, { notesWithPendingEdits, pagesWithPendingEdits, recentPages } from "./TrayPanel";
import type { Page } from "../lib/tauri";

const mockedInvoke = vi.mocked(invoke);

function page(id: string, title: string, lastModified: string, creationKind: string | null = null): Page {
  return {
    id,
    title,
    summary: null,
    content: "",
    entity_id: null,
    domain: null,
    source_memory_ids: [],
    version: 1,
    status: "active",
    creation_kind: creationKind,
    created_at: lastModified,
    last_compiled: lastModified,
    last_modified: lastModified,
  };
}

type Replies = Record<string, unknown>;

function serve(replies: Replies) {
  mockedInvoke.mockImplementation(async (command: string) => {
    if (command in replies) return replies[command];
    return undefined;
  });
}

function renderPanel() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TrayPanel />
    </QueryClientProvider>,
  );
}

const now = new Date();
const today = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 0, 30).toISOString();
const older = "2026-01-02T08:00:00Z";

const quietDay: Replies = {
  tray_panel_daemon_state: "up",
  list_pages: [
    page("p-old", "Old page", older),
    page("p-new", "Launch checklist", today),
    page("e-1", "Shadow entity", today, "entity"),
  ],
  list_pending_revisions: [],
  list_note_proposals: [],
  get_background_ai_enabled: true,
  list_new_material: { items: [], total: 0 },
};

beforeEach(() => {
  mockedInvoke.mockReset();
});

describe("recentPages", () => {
  it("leaves out entity pages and puts the newest first", () => {
    const pages = [page("a", "A", older), page("b", "B", today), page("e", "E", today, "entity")];
    expect(recentPages(pages).map((p) => p.id)).toEqual(["b", "a"]);
  });
});

describe("pagesWithPendingEdits", () => {
  it("counts each page once and skips memory revisions", () => {
    expect(
      pagesWithPendingEdits([
        { target_kind: "page", target_source_id: "p1" },
        { target_kind: "page", target_source_id: "p1" },
        { target_kind: "page", target_source_id: "p2" },
        { target_kind: "memory", target_source_id: "m1" },
      ]),
    ).toBe(2);
  });
});

describe("notesWithPendingEdits", () => {
  it("counts each vault note once", () => {
    expect(
      notesWithPendingEdits([
        { source_id: "v1", path: "Plans.md" },
        { source_id: "v1", path: "Plans.md" },
        { source_id: "v1", path: "Ideas/Plans.md" },
        { source_id: "v2", path: "Plans.md" },
      ]),
    ).toBe(3);
  });
});

describe("TrayPanel", () => {
  it("shows recent pages and stays quiet when nothing needs the user", async () => {
    serve(quietDay);
    renderPanel();

    expect(await screen.findByText("Launch checklist")).toBeTruthy();
    expect(screen.getByText("Old page")).toBeTruthy();
    expect(screen.queryByText("Shadow entity")).toBeNull();
    expect(screen.queryByTestId("tray-panel-needs-you")).toBeNull();
    expect(screen.getByTestId("tray-panel-status").textContent).toBe("Running");
    expect((await screen.findByTestId("tray-panel-steep")).textContent).toBe(
      "Steeping: Automatic, all caught up",
    );
  });

  it("asks for the user only when an AI edit waits on one of their pages", async () => {
    serve({
      ...quietDay,
      list_pending_revisions: [
        { target_kind: "page", target_source_id: "p1", revision_source_id: "r1", revision_content: "", source_agent: null, last_modified: 0 },
        { target_kind: "page", target_source_id: "p2", revision_source_id: "r2", revision_content: "", source_agent: null, last_modified: 0 },
      ],
    });
    renderPanel();

    const need = await screen.findByTestId("tray-panel-needs-you");
    expect(need.textContent).toContain("AI wants to edit 2 pages");
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(mockedInvoke).toHaveBeenCalledWith("tray_panel_open_main", { target: { kind: "review" } });
  });

  it("counts vault notes an AI wants to change and opens the home that lists them", async () => {
    serve({
      ...quietDay,
      list_note_proposals: [
        { id: "n1", source_id: "v1", path: "Plans.md", content: "", reason: "", agent: "claude-code", created_at: 0, base_hash: "" },
      ],
    });
    renderPanel();

    const need = await screen.findByTestId("tray-panel-note-suggestions");
    expect(need.textContent).toContain("AI wants to change 1 note in your vault");
    expect(screen.queryByTestId("tray-panel-needs-you")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(mockedInvoke).toHaveBeenCalledWith("tray_panel_open_main", { target: { kind: "home" } });
  });

  it("tells an On demand user what to type when material is waiting", async () => {
    serve({
      ...quietDay,
      get_background_ai_enabled: false,
      list_new_material: { items: [], total: 5, held: 1 },
    });
    renderPanel();

    expect((await screen.findByTestId("tray-panel-steep")).textContent).toBe(
      "Steeping: On demand, 4 ready, run /wenlan steep",
    );
  });

  it("opens a recent page in the main window", async () => {
    serve(quietDay);
    renderPanel();

    fireEvent.click(await screen.findByRole("button", { name: "Open Launch checklist" }));
    expect(mockedInvoke).toHaveBeenCalledWith("tray_panel_open_main", { target: { kind: "page", pageId: "p-new" } });
  });

  it("says plainly when Wenlan has stopped and offers a restart", async () => {
    serve({ ...quietDay, tray_panel_daemon_state: "down" });
    renderPanel();

    expect(await screen.findByTestId("tray-panel-daemon-down")).toBeTruthy();
    expect(screen.getByTestId("tray-panel-status").textContent).toBe("Stopped");
    expect(screen.queryByText("Launch checklist")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Restart" }));
    await waitFor(() => expect(mockedInvoke).toHaveBeenCalledWith("restart_daemon"));
  });

  it("has one Quit", async () => {
    serve(quietDay);
    renderPanel();

    await screen.findByText("Launch checklist");
    const quits = screen.getAllByRole("button").filter((button) => /quit/i.test(button.textContent ?? ""));
    expect(quits).toHaveLength(1);
    fireEvent.click(quits[0]);
    expect(mockedInvoke).toHaveBeenCalledWith("tray_panel_quit");
  });
});
