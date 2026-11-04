// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { listAllTags, patchDocumentTag, type IndexedFileInfo } from "../../../lib/tauri";
import { SelectedSourceActions } from "./SelectedSourceActions";
import { handToTool } from "../askAi/askInTool";

vi.mock("../../../lib/tauri", () => ({ listAllTags: vi.fn(), patchDocumentTag: vi.fn() }));
vi.mock("../../connect/connectionState", () => ({ useLiveAgents: () => ({ isSuccess: true, data: [] }) }));
vi.mock("../askAi/WritePages", () => ({ connectedTools: () => [{ family: "claude-code", name: "Claude Code" }, { family: "codex", name: "Codex" }] }));
vi.mock("../askAi/askInTool", () => ({ handToTool: vi.fn().mockResolvedValue("opened"), opensWithText: (family: string) => ["claude-code", "codex"].includes(family) }));

const sources: IndexedFileInfo[] = [
  { source: "file", source_id: "zotero::A", title: "Alpha", chunk_count: 1, last_modified: 0 },
  { source: "webpage", source_id: "web::B", title: "Beta", chunk_count: 1, last_modified: 0 },
];
function renderTags() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SelectedSourceActions spaceId="space-1" spaceName="Research" sources={sources} mode="tags" onClose={vi.fn()} /></QueryClientProvider>);
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(listAllTags).mockResolvedValue({ tags: ["human", "keep"], categories: [], document_categories: {}, document_tags: { "zotero::zotero::A": ["human", "keep"], "web::web::B": ["keep"] } });
  vi.mocked(patchDocumentTag).mockResolvedValue(undefined);
});

describe("SelectedSourceActions", () => {
  it("uses the atomic patch API and leaves unrelated tags to the backend", async () => {
    renderTags();
    fireEvent.change(await screen.findByLabelText("Tag"), { target: { value: "reviewed" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    await waitFor(() => expect(patchDocumentTag).toHaveBeenCalledTimes(2));
    expect(patchDocumentTag).toHaveBeenNthCalledWith(1, "file", "zotero::A", "reviewed", "add");
    expect(patchDocumentTag).toHaveBeenNthCalledWith(2, "webpage", "web::B", "reviewed", "add");
  });

  it("sends a scoped atomic remove without replacing the remaining tags", async () => {
    renderTags();
    fireEvent.change(await screen.findByLabelText("Action"), { target: { value: "remove" } });
    fireEvent.change(screen.getByLabelText("Tag"), { target: { value: "human" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    await waitFor(() => expect(patchDocumentTag).toHaveBeenCalledTimes(2));
    expect(patchDocumentTag).toHaveBeenCalledWith("file", "zotero::A", "human", "remove");
  });

  it("reports partial failure and retries only the failed source", async () => {
    vi.mocked(patchDocumentTag).mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(undefined).mockResolvedValueOnce(undefined);
    renderTags();
    fireEvent.change(await screen.findByLabelText("Tag"), { target: { value: "reviewed" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Tag operation succeeded on 1 of 2 sources; 1 failed");
    expect(screen.getByRole("alert")).toHaveTextContent("Alpha");
    fireEvent.click(screen.getByRole("button", { name: "Retry failed sources" }));
    await waitFor(() => expect(patchDocumentTag).toHaveBeenCalledTimes(3));
    expect(patchDocumentTag).toHaveBeenNthCalledWith(3, "file", "zotero::A", "reviewed", "add");
    expect(screen.getByRole("status")).toHaveTextContent("Tag operation succeeded on 2 sources.");
  });

  it("does not replace concurrently added tags", async () => {
    renderTags();
    fireEvent.change(await screen.findByLabelText("Tag"), { target: { value: "reviewed" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    await waitFor(() => expect(patchDocumentTag).toHaveBeenCalledTimes(2));
    expect(patchDocumentTag).toHaveBeenNthCalledWith(1, "file", "zotero::A", "reviewed", "add");
    expect(patchDocumentTag).not.toHaveBeenCalledWith("file", "zotero::A", expect.any(Array));
  });

  it("allows edits when the tag suggestion inventory cannot load", async () => {
    vi.mocked(listAllTags).mockRejectedValueOnce(new Error("offline"));
    renderTags();
    fireEvent.change(await screen.findByLabelText("Tag"), { target: { value: "reviewed" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    await waitFor(() => expect(patchDocumentTag).toHaveBeenCalledTimes(2));
    expect(patchDocumentTag).toHaveBeenCalledWith("file", "zotero::A", "reviewed", "add");
  });

  it("hands the prompt to the tool currently selected in the real ToolPicker", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><SelectedSourceActions spaceId="space-1" spaceName="Research" sources={sources} mode="organize" onClose={vi.fn()} /></QueryClientProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "Use another tool" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "Open Codex with prompt" }));
    await waitFor(() => expect(handToTool).toHaveBeenCalledWith("codex", expect.stringContaining('"space":"Research"')));
  });

  it("restores the optional instruction draft after the source view remounts", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const props = { spaceId: "space-1", spaceName: "Research", sources, mode: "organize" as const, onClose: vi.fn() };
    const view = render(<QueryClientProvider client={client}><SelectedSourceActions {...props} /></QueryClientProvider>);
    fireEvent.change(screen.getByLabelText("Optional instructions"), { target: { value: "Compare the study limitations" } });
    view.unmount();
    render(<QueryClientProvider client={client}><SelectedSourceActions {...props} /></QueryClientProvider>);
    expect(screen.getByLabelText("Optional instructions")).toHaveValue("Compare the study limitations");
  });
});
