import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getSourceLifecycle, setSourceLifecycle, type SourceLifecycleResponse } from "../../../lib/sourceLifecycleApi";
import { SourceLifecycleControl } from "./SourceLifecycleControl";
import { SourceLifecycleNotice } from "./SourceLifecycleNotice";
import { getPage, type Page } from "../../../lib/tauri";

vi.mock("../../../lib/sourceLifecycleApi", async original => ({ ...(await original<typeof import("../../../lib/sourceLifecycleApi")>()), getSourceLifecycle: vi.fn(), setSourceLifecycle: vi.fn() }));
vi.mock("../../../lib/tauri", async original => ({ ...(await original<typeof import("../../../lib/tauri")>()), getPage: vi.fn() }));
const response = (revision = 0, state: SourceLifecycleResponse["record"]["state"] = "active"): SourceLifecycleResponse => ({ record: { space_id: "space-id", source: "file", source_id: "paper", revision, state, reason: state === "active" ? "" : "Retracted by publisher", actor: "human", updated_at: 0 }, history: [], impacts: [] });
function renderControls() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SourceLifecycleControl source="file" sourceId="paper" space="Research" /><SourceLifecycleNotice source="file" sourceId="paper" space="Research" /></QueryClientProvider>);
}
beforeEach(() => { vi.resetAllMocks(); vi.mocked(getSourceLifecycle).mockResolvedValue(response()); vi.mocked(getPage).mockResolvedValue({ id: "page-1", title: "Research findings", space: "Research" } as Page); });
describe("source lifecycle", () => {
  it("shows no active banner and saves a scoped, reasoned decision with its observed revision", async () => {
    const updated = { ...response(1, "retracted"), impacts: [{ page_id: "page-1", page_version: 2, source_revision: 3, source_ids: ["paper"], occurrences: [1], reason: "Retracted" }] };
    vi.mocked(setSourceLifecycle).mockResolvedValue(updated);
    renderControls();
    fireEvent.click(screen.getByRole("button", { name: "Source options" }));
    fireEvent.click(screen.getByRole("button", { name: "Change source status" }));
    await screen.findByText("Current status:", { exact: false });
    const save = screen.getByRole("button", { name: "Save status" });
    expect(save).toBeDisabled();
    fireEvent.change(screen.getByLabelText("New status"), { target: { value: "retracted" } });
    fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "Retracted by publisher" } });
    fireEvent.click(save);
    expect(await screen.findByText("Affected pages: 1")).toBeInTheDocument();
    expect(await screen.findByText("Research findings")).toBeInTheDocument();
    expect(screen.queryByText("page-1")).not.toBeInTheDocument();
    expect(setSourceLifecycle).toHaveBeenCalledWith({ space: "Research", source: "file", source_id: "paper", expected_revision: 0, state: "retracted", reason: "Retracted by publisher" });
    expect(await screen.findByText("Marked retracted: Retracted by publisher")).toBeInTheDocument();
  });
  it("shows scoped impact titles before saving and hides foreign or unavailable page identities", async () => {
    const impact = (page_id: string) => ({page_id, page_version: 1, source_revision: 1, source_ids: ["paper"], occurrences: [1], reason: ""});
    vi.mocked(getSourceLifecycle).mockResolvedValue({...response(), preview_impacts: [impact("own-id"), impact("foreign-id"), impact("failed-id")]});
    vi.mocked(getPage).mockImplementation(async id => { if (id === "failed-id") throw new Error("unavailable"); return {id, title: id === "own-id" ? "Visible research" : "Foreign secret", space: id === "own-id" ? "Research" : "Other"} as Page; });
    renderControls(); fireEvent.click(screen.getByRole("button", {name: "Source options"})); fireEvent.click(screen.getByRole("button", {name: "Change source status"}));
    expect(await screen.findByText("Visible research")).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByText("Page unavailable")).toHaveLength(2));
    expect(screen.queryByText("Foreign secret")).not.toBeInTheDocument();
    for (const id of ["own-id", "foreign-id", "failed-id"]) expect(screen.queryByText(id)).not.toBeInTheDocument();
    expect(setSourceLifecycle).not.toHaveBeenCalled();
  });
  it("refreshes on a rejected write and does not claim it was saved", async () => {
    vi.mocked(getSourceLifecycle).mockResolvedValueOnce(response()).mockResolvedValue(response(2, "excluded"));
    vi.mocked(setSourceLifecycle).mockRejectedValue(new Error("conflict"));
    renderControls();
    fireEvent.click(screen.getByRole("button", { name: "Source options" }));
    fireEvent.click(screen.getByRole("button", { name: "Change source status" }));
    await screen.findByLabelText("Reason");
    fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "Evidence changed" } });
    fireEvent.click(screen.getByRole("button", { name: "Save status" }));
    expect(await screen.findByText(/Status could not be saved/)).toBeInTheDocument();
    await waitFor(() => expect(getSourceLifecycle).toHaveBeenCalledTimes(2));
    expect(await screen.findByText("Current reason: Retracted by publisher")).toBeInTheDocument();
    expect(screen.queryByText(/Source status saved/)).not.toBeInTheDocument();
  });
});
