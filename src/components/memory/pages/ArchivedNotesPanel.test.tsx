import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { getPage, listPagesExplicitBrowse, restorePage, RestorePageApiError, type Page, type RestorePageResponse } from "../../../lib/tauri";
import { ArchivedNotesPanel } from "./ArchivedNotesPanel";

vi.mock("../../../lib/tauri", async original => ({
  ...await original<typeof import("../../../lib/tauri")>(),
  getPage: vi.fn(), listPagesExplicitBrowse: vi.fn(), listPages: vi.fn(), restorePage: vi.fn(),
}));
vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), warning: vi.fn() }) }));
function note(overrides: Partial<Page> = {}): Page {
  return { id: "archived-one", title: "Archived one", content: "\n\nOriginal **body**  \n\n", summary: null, entity_id: null, space: null, source_memory_ids: [], version: 2, status: "archived", created_at: "", last_compiled: "", last_modified: "", creation_kind: "authored", ...overrides } as Page;
}
let rows: Page[];
function renderPanel() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenPage = vi.fn();
  return { client, onOpenPage, ...render(<QueryClientProvider client={client}><ArchivedNotesPanel onOpenPage={onOpenPage} /></QueryClientProvider>) };
}
beforeEach(() => {
  vi.clearAllMocks();
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.open = true; } });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value: function (this: HTMLDialogElement) { this.open = false; } });
  rows = [note()];
  vi.mocked(listPagesExplicitBrowse).mockImplementation(async () => rows);
  vi.mocked(getPage).mockImplementation(async () => rows[0] ?? null);
  vi.mocked(restorePage).mockImplementation(async (id, expectedVersion, title) => {
    rows = rows.filter(row => row.id !== id);
    return { id, title: title ?? "Archived one", version: expectedVersion + 1, status: "active", projection_status: "synced" };
  });
});
afterEach(cleanup);

describe("Archived pages", () => {
  it("browses all archived pages, filters titles and opens the original page", async () => {
    rows = [note(), note({ id: "two", title: "中文筆記" }), note({ id: "entity", title: "Entity shadow", creation_kind: "entity" })];
    const { onOpenPage } = renderPanel();
    await screen.findByRole("button", { name: "Archived one" });
    expect(listPagesExplicitBrowse).toHaveBeenCalledWith("archived", undefined, 500, 0);
    expect(screen.queryByRole("button", { name: "Entity shadow" })).not.toBeInTheDocument();
    await userEvent.type(screen.getByRole("searchbox", { name: "Search archived pages" }), "中文");
    expect(screen.queryByRole("button", { name: "Archived one" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "中文筆記" }));
    expect(onOpenPage).toHaveBeenCalledWith(rows[1]);
  });
  it("shows an unavailable state instead of an empty library and retries", async () => {
    vi.mocked(listPagesExplicitBrowse).mockRejectedValueOnce(new Error("offline"));
    renderPanel();
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be loaded");
    expect(screen.queryByText("No archived pages.")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByRole("button", { name: "Archived one" });
  });
  it("keeps a conflicting restore open so another title restores the same ID", async () => {
    vi.mocked(restorePage).mockRejectedValueOnce(new RestorePageApiError("page title already exists", "page_title_taken"));
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Another page already has this title. Choose a different title.");
    const input = screen.getByRole("textbox", { name: "Page title" });
    await userEvent.clear(input); await userEvent.type(input, "Recovered one");
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(restorePage).toHaveBeenLastCalledWith("archived-one", 2, "Recovered one"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(toast.success).toHaveBeenCalledWith("Page restored.");
  });
  it("says what a restore under a new title did to links, as a rename does", async () => {
    vi.mocked(restorePage).mockImplementation(async (id, expectedVersion, title) => {
      rows = [];
      return { id, title: title ?? "Archived one", version: expectedVersion + 1, status: "active", projection_status: "synced", links_updated: 2, links_pending_review: 1 };
    });
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    const input = screen.getByRole("textbox", { name: "Page title" });
    await userEvent.clear(input); await userEvent.type(input, "Recovered one");
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(toast.success).toHaveBeenCalledWith("Page restored.");
    expect(toast).toHaveBeenCalledWith("Updated links in 2 pages. 1 page asks you first, so its link update is waiting for your review.");
  });
  it("restores under the same title without a link notice", async () => {
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(toast).not.toHaveBeenCalled();
  });
  it("reloads a stale version without discarding the user's chosen restore title", async () => {
    vi.mocked(restorePage).mockRejectedValueOnce(new RestorePageApiError("page version conflict", "page_version_conflict", 9));
    vi.mocked(getPage).mockResolvedValue(note({ version: 9, content: "New durable body" }));
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    const input = screen.getByRole("textbox", { name: "Page title" });
    await userEvent.clear(input); await userEvent.type(input, "Chosen name");
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await screen.findByRole("alert");
    await userEvent.click(screen.getByRole("button", { name: "Reload" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(input).toHaveValue("Chosen name");
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(restorePage).toHaveBeenLastCalledWith("archived-one", 9, "Chosen name"));
  });
  it("blocks submission and dialog Escape while a Chinese title is composing", async () => {
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    const input = screen.getByRole("textbox", { name: "Page title" });
    const dialog = screen.getByRole("dialog");
    fireEvent.compositionStart(input);
    expect(screen.getByRole("button", { name: "Restore page" })).toBeDisabled();
    fireEvent.keyDown(input, { key: "Escape", isComposing: true });
    fireEvent(dialog, new Event("cancel", { bubbles: false, cancelable: true }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.submit(dialog.querySelector("form")!);
    expect(restorePage).not.toHaveBeenCalled();
    fireEvent.compositionEnd(input);
    await userEvent.clear(input); await userEvent.type(input, "中文筆記");
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(restorePage).toHaveBeenCalledWith("archived-one", 2, "中文筆記"));
  });
  it("restores a source page under its fixed title, including authored boundary whitespace", async () => {
    rows = [note({ title: " Legacy source ", creation_kind: "source" })];
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Legacy source" }));
    expect(screen.getByRole("textbox", { name: "Page title" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(restorePage).toHaveBeenCalledWith("archived-one", 2, undefined));
  });
  it.each(["pending", "hidden"] as const)("reports a successful DB restore with %s file state accurately", async status => {
    vi.mocked(restorePage).mockImplementation(async id => {
      rows = [];
      return { id, title: "Archived one", version: 3, status: "active", projection_status: status } satisfies RestorePageResponse;
    });
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(toast.warning).toHaveBeenCalledWith(expect.stringContaining("Page restored."));
    expect(toast.success).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Archived one" })).not.toBeInTheDocument();
  });
  it("distinguishes identical titles in different Spaces and restores the intended ID", async () => {
    rows = [note({ id: "work-note", title: "Same title", space: "work" }), note({ id: "home-note", title: "Same title", space: "home" })];
    const { onOpenPage } = renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Same title · home" }));
    expect(onOpenPage).toHaveBeenCalledWith(rows[1]);
    await userEvent.click(screen.getByRole("button", { name: "Restore Same title · work" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("work");
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    await waitFor(() => expect(restorePage).toHaveBeenCalledWith("work-note", 2, "Same title"));
  });
  it.each([
    ["page_title_taken", "authored", "Another page already has this title. Choose a different title."],
    ["page_title_taken", "source", "Another page already has this title. Rename that page, then restore this one."],
    ["page_not_archived", "authored", "This page changed since you opened it. Reload it, then restore."],
    ["page_title_fixed", "authored", "Source and imported pages keep their original title."],
    ["page_not_restorable", "authored", "This page cannot be restored."],
  ] as const)("maps the typed %s refusal for a %s page, never the daemon's English text", async (code, kind, message) => {
    rows = [note({ creation_kind: kind })];
    vi.mocked(restorePage).mockRejectedValueOnce(new RestorePageApiError("page title already exists", code));
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
  });
  it("shows the generic message for an untyped failure even when its text names a known refusal", async () => {
    vi.mocked(restorePage).mockRejectedValueOnce(new Error("HTTP 409: page title already exists"));
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore page" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The page could not be restored. Try again.");
  });
  it("offers a short Restore button per row, named after its page", async () => {
    renderPanel();
    const button = await screen.findByRole("button", { name: "Restore Archived one" });
    expect(button).toHaveTextContent(/^Restore$/);
  });
});
