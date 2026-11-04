import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as tauri from "../../../lib/tauri";
import ZoteroSourceDialog from "./ZoteroSourceDialog";

vi.mock("../../../lib/tauri");

function renderDialog(fixedSpace?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  render(<QueryClientProvider client={client}><ZoteroSourceDialog fixedSpace={fixedSpace} onClose={vi.fn()} /></QueryClientProvider>);
  return { client, invalidate };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(tauri.listZoteroCollections).mockResolvedValue({ server_id: "local-1", collections: [
    { key: "A1", name: "Articles", item_count: 8 }, { key: "B2", name: "Books", item_count: 3 },
  ] });
  vi.mocked(tauri.listSpaces).mockResolvedValue([
    { id: "space-1", name: "Research", description: null, suggested: false, starred: false, sort_order: 0, memory_count: 0, entity_count: 0, created_at: 0, updated_at: 0 },
    { id: "space-2", name: "Reading", description: null, suggested: false, starred: false, sort_order: 1, memory_count: 0, entity_count: 0, created_at: 0, updated_at: 0 },
  ]);
  vi.mocked(tauri.addZoteroSource).mockResolvedValue({ id: "zotero-src", source_type: "zotero", path: "zotero-local/instance/users/0/collections/B2", status: "Active", last_sync: null, file_count: 0, memory_count: 0 });
  vi.mocked(tauri.syncRegisteredSource).mockResolvedValue({ files_found: 3, ingested: 2, skipped: 1, errors: 0 });
});

describe("ZoteroSourceDialog", () => {
  it("registers only the selected collection in the chosen destination and reports sync stats", async () => {
    const { invalidate } = renderDialog();
    await screen.findByText("Connected to local Zotero");
    fireEvent.change(screen.getByLabelText("Collection"), { target: { value: "B2" } });
    fireEvent.change(screen.getByLabelText("Destination Space"), { target: { value: "Reading" } });
    fireEvent.click(screen.getByRole("button", { name: "Import collection" }));

    expect(await screen.findByText("Imported 2; skipped 1; errors 0.")).toBeInTheDocument();
    expect(tauri.addZoteroSource).toHaveBeenCalledExactlyOnceWith("B2", "local-1", "Reading");
    expect(tauri.syncRegisteredSource).toHaveBeenCalledExactlyOnceWith("zotero-src");
    expect(tauri.listSpaces).toHaveBeenCalledOnce();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["registeredSources"] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["space-sources"] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["indexedFiles"] });
  });

  it("keeps collection selection and an actionable error when sync fails, then permits an idempotent retry", async () => {
    vi.mocked(tauri.syncRegisteredSource).mockRejectedValueOnce(new Error("local connection lost"));
    renderDialog("Research");
    await screen.findByText("Connected to local Zotero");
    fireEvent.change(screen.getByLabelText("Collection"), { target: { value: "B2" } });
    fireEvent.click(screen.getByRole("button", { name: "Import collection" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("local connection lost");
    expect(screen.getByLabelText("Collection")).toHaveValue("B2");
    expect(screen.getByRole("button", { name: "Retry import" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Retry import" }));
    await screen.findByText("Imported 2; skipped 1; errors 0.");
    await waitFor(() => expect(tauri.addZoteroSource).toHaveBeenCalledTimes(2));
    expect(tauri.addZoteroSource).toHaveBeenLastCalledWith("B2", "local-1", "Research");
  });

  it("shows partial import errors as an alert while still allowing the result to close", async () => {
    vi.mocked(tauri.syncRegisteredSource).mockResolvedValue({ files_found: 3, ingested: 1, skipped: 1, errors: 1 });
    renderDialog("Research");
    await screen.findByText("Connected to local Zotero");
    fireEvent.click(screen.getByRole("button", { name: "Import collection" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Imported 1; skipped 1; errors 1.");
    const dialog = within(screen.getByRole("dialog"));
    const closeButtons = dialog.getAllByRole("button", { name: "Close" });
    expect(closeButtons[closeButtons.length - 1]).toBeEnabled();
  });

  it("shows local Zotero offline and retries collection discovery", async () => {
    vi.mocked(tauri.listZoteroCollections).mockRejectedValueOnce(new Error("Zotero is closed"));
    renderDialog("Research");
    expect(await screen.findByText("Local Zotero is unavailable.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("Connected to local Zotero")).toBeInTheDocument();
    expect(tauri.listZoteroCollections).toHaveBeenCalledTimes(2);
  });

  it("does not offer an import when local Zotero has no collections", async () => {
    vi.mocked(tauri.listZoteroCollections).mockResolvedValue({ server_id: null, collections: [] });
    renderDialog("Research");
    expect(await screen.findByText("No collections are available in the local Zotero library.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import collection" })).toBeDisabled();
    expect(tauri.addZoteroSource).not.toHaveBeenCalled();
  });
});
