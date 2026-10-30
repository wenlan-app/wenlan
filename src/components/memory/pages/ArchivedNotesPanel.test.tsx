import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { getPage, listPagesExplicitBrowse, restorePage, type Page, type RestorePageResponse } from "../../../lib/tauri";
import { ArchivedNotesPanel } from "./ArchivedNotesPanel";

vi.mock("../../../lib/tauri", () => ({ getPage: vi.fn(), listPagesExplicitBrowse: vi.fn(), listPages: vi.fn(), restorePage: vi.fn() }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), warning: vi.fn() } }));
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

describe("Archived notes recovery", () => {
  it("browses all archived knowledge notes, filters titles and opens the original note", async () => {
    rows = [note(), note({ id: "two", title: "中文筆記" }), note({ id: "entity", title: "Entity shadow", creation_kind: "entity" })];
    const { onOpenPage } = renderPanel();
    await screen.findByRole("button", { name: "Archived one" });
    expect(listPagesExplicitBrowse).toHaveBeenCalledWith("archived", undefined, 500, 0);
    expect(screen.queryByRole("button", { name: "Entity shadow" })).not.toBeInTheDocument();
    await userEvent.type(screen.getByRole("searchbox", { name: "Search archived notes" }), "中文");
    expect(screen.queryByRole("button", { name: "Archived one" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "中文筆記" }));
    expect(onOpenPage).toHaveBeenCalledWith(rows[1]);
  });
  it("shows an unavailable state instead of an empty library and retries", async () => {
    vi.mocked(listPagesExplicitBrowse).mockRejectedValueOnce(new Error("offline"));
    renderPanel();
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be loaded");
    expect(screen.queryByText("No archived notes.")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByRole("button", { name: "Archived one" });
  });
  it("keeps a conflicting restore open so another title restores the same ID", async () => {
    vi.mocked(restorePage).mockRejectedValueOnce(new Error("HTTP 409: page title already exists"));
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Choose another name");
    const input = screen.getByRole("textbox", { name: "Note title" });
    await userEvent.clear(input); await userEvent.type(input, "Recovered one");
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await waitFor(() => expect(restorePage).toHaveBeenLastCalledWith("archived-one", 2, "Recovered one"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(toast.success).toHaveBeenCalledWith("Note restored.");
  });
  it("reloads a stale version without discarding the user's chosen restore title", async () => {
    vi.mocked(restorePage).mockRejectedValueOnce(new Error("HTTP 409: page version conflict"));
    vi.mocked(getPage).mockResolvedValue(note({ version: 9, content: "New durable body" }));
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    const input = screen.getByRole("textbox", { name: "Note title" });
    await userEvent.clear(input); await userEvent.type(input, "Chosen name");
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await screen.findByRole("alert");
    await userEvent.click(screen.getByRole("button", { name: "Reload note" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(input).toHaveValue("Chosen name");
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await waitFor(() => expect(restorePage).toHaveBeenLastCalledWith("archived-one", 9, "Chosen name"));
  });
  it("blocks submission and dialog Escape while a Chinese title is composing", async () => {
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    const input = screen.getByRole("textbox", { name: "Note title" });
    const dialog = screen.getByRole("dialog");
    fireEvent.compositionStart(input);
    expect(screen.getByRole("button", { name: "Restore note" })).toBeDisabled();
    fireEvent.keyDown(input, { key: "Escape", isComposing: true });
    fireEvent(dialog, new Event("cancel", { bubbles: false, cancelable: true }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.submit(dialog.querySelector("form")!);
    expect(restorePage).not.toHaveBeenCalled();
    fireEvent.compositionEnd(input);
    await userEvent.clear(input); await userEvent.type(input, "中文筆記");
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await waitFor(() => expect(restorePage).toHaveBeenCalledWith("archived-one", 2, "中文筆記"));
  });
  it("restores a legacy source title unchanged, including authored boundary whitespace", async () => {
    rows = [note({ title: " Legacy source ", creation_kind: "source" })];
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Legacy source" }));
    expect(screen.getByRole("textbox", { name: "Note title" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await waitFor(() => expect(restorePage).toHaveBeenCalledWith("archived-one", 2, undefined));
  });
  it.each(["pending", "hidden"] as const)("reports a successful DB restore with %s file state accurately", async status => {
    vi.mocked(restorePage).mockImplementation(async id => {
      rows = [];
      return { id, title: "Archived one", version: 3, status: "active", projection_status: status } satisfies RestorePageResponse;
    });
    renderPanel();
    await userEvent.click(await screen.findByRole("button", { name: "Restore Archived one" }));
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(toast.warning).toHaveBeenCalledWith(expect.stringContaining("Restored in Wenlan."));
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
    await userEvent.click(screen.getByRole("button", { name: "Restore note" }));
    await waitFor(() => expect(restorePage).toHaveBeenCalledWith("work-note", 2, "Same title"));
  });

});
