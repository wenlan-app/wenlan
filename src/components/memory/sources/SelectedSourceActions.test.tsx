// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { listPages, listAllTags, patchDocumentTag, type IndexedFileInfo } from "../../../lib/tauri";
import { SelectedSourceActions } from "./SelectedSourceActions";
import { createResearchRun, getResearchRun, getResearchRules, mergeResearchTags, resumeResearchRun, saveResearchRules } from "../../../lib/researchApi";
import { connectedTools } from "../askAi/WritePages";
import { handToTool } from "../askAi/askInTool";

vi.mock("../../../lib/researchApi", () => ({ mergeResearchTags: vi.fn(), resumeResearchRun: vi.fn(), getResearchRules: vi.fn(), getResearchRun: vi.fn(), saveResearchRules: vi.fn(), createResearchRun: vi.fn() }));
vi.mock("../../../lib/tauri", () => ({ getPage: vi.fn(), listPages: vi.fn(), listAllTags: vi.fn(), patchDocumentTag: vi.fn() }));
vi.mock("../../connect/connectionState", () => ({ useLiveAgents: () => ({ isSuccess: true, data: [] }) }));
vi.mock("../askAi/WritePages", () => ({ connectedTools: vi.fn() }));
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
  sessionStorage.clear();
  vi.mocked(listPages).mockResolvedValue([]);
  vi.mocked(connectedTools).mockReturnValue([{ family: "claude-code", name: "Claude Code" }, { family: "codex", name: "Codex" }] as ReturnType<typeof connectedTools>);
  vi.mocked(saveResearchRules).mockResolvedValue({ space_id: "space-1", revision: 1, name: "", instructions: "New rule", content_digest: "new" });
  vi.mocked(getResearchRules).mockResolvedValue({ space_id: "space-1", revision: 0, name: "", instructions: "", content_digest: "digest" });
  vi.mocked(createResearchRun).mockResolvedValue({ id: "run-1", space_id: "space-1", revision: 1, rules: { space_id: "space-1", revision: 0, name: "", instructions: "", content_digest: "digest" }, items: [], target_page_id: null, target_page_version: null, result_page_version: null, pending_revision_id: null, state: "prepared", created_at: 1, updated_at: 1 });
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

  it("merges synonyms in one scoped call and reports an unconfirmed response", async () => {
    vi.mocked(mergeResearchTags).mockRejectedValueOnce(new Error("conflict")).mockResolvedValueOnce({ updated: 2 });
    renderTags();
    fireEvent.change(await screen.findByLabelText("Action"), { target: { value: "merge" } });
    fireEvent.change(screen.getByLabelText("Tag"), { target: { value: "retrieval" } });
    fireEvent.change(screen.getByLabelText("Merge into"), { target: { value: "rag" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not confirm the merge");
    expect(patchDocumentTag).not.toHaveBeenCalled();
    expect(mergeResearchTags).toHaveBeenCalledWith({ space: "Research", sources: sources.map(({source,source_id,title}) => ({source,source_id,title})), from_tag: "retrieval", into_tag: "rag" });
    fireEvent.click(screen.getByRole("button", { name: "Apply to 2 sources" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Tag operation succeeded on 2 sources"));
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
    await waitFor(() => expect(handToTool).toHaveBeenCalledWith("codex", expect.stringContaining('run-1')));
  });

  it("saves scoped defaults and offers a copy handoff without a connected AI tool", async () => {
    vi.mocked(connectedTools).mockReturnValue([]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><SelectedSourceActions spaceId="space-1" spaceName="Research" sources={sources} mode="organize" onClose={vi.fn()} /></QueryClientProvider>);
    const button = await screen.findByRole("button", { name: "Copy prompt for your AI tool" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.change(screen.getByLabelText("Optional instructions"), { target: { value: "New rule" } });
    fireEvent.click(screen.getByLabelText("Use these instructions for this Space"));
    fireEvent.click(button);
    await waitFor(() => expect(saveResearchRules).toHaveBeenCalledWith({ space: "Research", expected_revision: 0, name: "", instructions: "New rule" }));
    await waitFor(() => expect(handToTool).toHaveBeenCalledWith("other", expect.stringContaining("run-1")));
  });

  it("does not hand off if persisting the run fails", async () => {
    vi.mocked(createResearchRun).mockRejectedValueOnce(new Error("offline"));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><SelectedSourceActions spaceId="space-1" spaceName="Research" sources={sources} mode="organize" onClose={vi.fn()} /></QueryClientProvider>);
    const button = await screen.findByRole("button", { name: "Open Claude Code with prompt" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not prepare the run");
    expect(handToTool).not.toHaveBeenCalled();
  });

  it("resumes cancelled progress only on handoff and keeps its original instructions", async () => {
    const saved = await createResearchRun({space:"Research",sources:[],instructions:null,target_page_id:null});
    const run = { ...saved, state: "cancelled" as const, rules: { ...saved.rules, instructions: "Original rules" }, items: sources.map(source => ({...source,coverage:"pending" as const,content_digest:"d",decision_revision:0,next_offset:null,error:null})) };
    vi.mocked(getResearchRun).mockResolvedValue(run);
    vi.mocked(resumeResearchRun).mockResolvedValue({...run,state:"in_progress",revision:2});
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><SelectedSourceActions spaceId="space-1" spaceName="Research" sources={sources} mode="organize" resumeRun={run} currentSelectionCount={0} onClose={vi.fn()} /></QueryClientProvider>);
    expect(screen.getByLabelText("Previous instructions")).toHaveValue("Original rules");
    expect(screen.queryByText(/Current selection: 0/)).not.toBeInTheDocument();
    expect(resumeResearchRun).not.toHaveBeenCalled();
    const button = await screen.findByRole("button",{name:"Open Claude Code with prompt"});
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    await waitFor(() => expect(resumeResearchRun).toHaveBeenCalledWith("Research","run-1",1));
    await waitFor(() => expect(handToTool).toHaveBeenCalled());
    vi.mocked(getResearchRun).mockResolvedValue({...run,state:"in_progress",revision:2});
    fireEvent.click(screen.getByRole("button",{name:"Open Claude Code with prompt"}));
    await waitFor(() => expect(handToTool).toHaveBeenCalledTimes(2));
    expect(resumeResearchRun).toHaveBeenCalledTimes(1);
  });

  it("updates the prior research page and makes changed selection visible before handoff", async () => {
    const saved = await createResearchRun({space:"Research",sources:[],instructions:null,target_page_id:null});
    vi.mocked(createResearchRun).mockClear();
    const run = { ...saved, state: "completed" as const, target_page_id: "existing", items: [{...sources[0],coverage:"complete" as const,content_digest:"d",decision_revision:0,next_offset:null,error:null}, {...sources[1], source_id:"older",title:"Previous paper",coverage:"complete" as const,content_digest:"d",decision_revision:0,next_offset:null,error:null}] };
    vi.mocked(listPages).mockResolvedValue([{id:"existing",title:"Retrieval review",space:"Research"}, {id:"foreign",title:"Other Space",space:"Private"}] as Awaited<ReturnType<typeof listPages>>);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><SelectedSourceActions spaceId="space-1" spaceName="Research" sources={sources} previousRuns={[run]} mode="organize" onClose={vi.fn()} /></QueryClientProvider>);
    expect(await screen.findByLabelText("Write to")).toHaveValue("existing");
    expect(screen.queryByRole("option",{name:"Other Space"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Compared with the last run: 1 added, 1 no longer selected"));
    expect(screen.getByText("Previous paper")).toBeVisible();
    fireEvent.click(screen.getByRole("button",{name:"Open Claude Code with prompt"}));
    await waitFor(() => expect(createResearchRun).toHaveBeenCalledWith(expect.objectContaining({target_page_id:"existing",space:"Research",sources:sources.map(({source,source_id,title}) => ({source,source_id,title}))})));
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
