import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { capturePageExport, getPageExportBundle, listPageExportVersions, type PageExportBundle, type PageExportVersion } from "../../../lib/pageExportApi";
import PageExportDialog from "./PageExportDialog";
vi.mock("../../../lib/pageExportApi");
const base: PageExportBundle = { id: "current", digest: "current-digest", kind: "saved", page_id: "page-1", page_incarnation: "inc", space_id: "space", version: 4, export_revision: 2, source_revision: 1, title: "Current title", content: "Current text", last_modified: "2026-11-03T12:00:00Z", captured_at: 1793707200000, citations: [], sources: [], bibliography: [], references_available: true, pending_revision_id: null };
const old: PageExportBundle = { ...base, id: "old", digest: "old-digest", version: 2, title: "Old title", content: "Old claim [1]", citations: [{ occurrence: 1, marker: 1, source_kind: "memory", locator: "old-locator", score: 1, status: "verified", scope: "sentence" }], sources: [{locator:"old-locator",source_kind:"file",source_id:"paper",title:"Old paper",source_agent:"zotero",url:null,text_digest:null,historical:false}], bibliography: [{source_id:"paper",title:"Old paper",bibtex:"@article{old, title={Old paper}}",availability:"available"}] };
const pending: PageExportBundle = { ...old, id: "pending", digest: "pending-digest", kind: "pending_draft", pending_revision_id: "revision", content: "Pending text" };
const version = (bundle: PageExportBundle): PageExportVersion => ({bundle_id:bundle.id,kind:bundle.kind,version:bundle.version,export_revision:bundle.export_revision,title:bundle.title,saved_at:bundle.last_modified,captured_at:bundle.captured_at,references_available:true,bibliography_available:true,unavailable_reason:null});
function wrap() { return render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><PageExportDialog pageId="page-1" onClose={vi.fn()} /></QueryClientProvider>); }
beforeEach(() => { vi.resetAllMocks(); vi.mocked(capturePageExport).mockResolvedValue(base); vi.mocked(listPageExportVersions).mockResolvedValue({versions:[version(base),version(old),version(pending),{...version(old),bundle_id:null,kind:"legacy_unavailable",references_available:false}]}); vi.mocked(getPageExportBundle).mockImplementation(async (_page,id) => id === "old" ? old : id === "pending" ? pending : base); });
describe("immutable page export", () => {
  it("defaults to current saved, exports an old bundle with its own references, and disables legacy", async () => {
    const {save} = await import("@tauri-apps/plugin-dialog"); const {writeTextFile} = await import("@tauri-apps/plugin-fs"); vi.mocked(save).mockResolvedValue("/tmp/old.md"); wrap();
    const select = await screen.findByLabelText("Version"); expect(select).toHaveValue("current");
    expect(screen.getByRole("option",{name:/References were not retained/})).toBeDisabled();
    fireEvent.change(select,{target:{value:"old"}}); await waitFor(() => expect(screen.getByRole("button",{name:"Export"})).toBeEnabled()); fireEvent.click(screen.getByRole("button",{name:"Export"}));
    await waitFor(() => expect(writeTextFile).toHaveBeenCalledOnce()); const text = vi.mocked(writeTextFile).mock.calls[0][1]; expect(text).toContain("Old claim"); expect(text).toContain("Old paper"); expect(text).not.toContain("Current text"); expect(getPageExportBundle).toHaveBeenCalledWith("page-1","old");
  });
  it("labels pending text as not adopted", async () => { const {save} = await import("@tauri-apps/plugin-dialog"); const {writeTextFile} = await import("@tauri-apps/plugin-fs"); vi.mocked(save).mockResolvedValue("/tmp/pending.md"); wrap(); fireEvent.change(await screen.findByLabelText("Version"),{target:{value:"pending"}}); await waitFor(() => expect(screen.getByRole("button",{name:"Export"})).toBeEnabled()); fireEvent.click(screen.getByRole("button",{name:"Export"})); await waitFor(() => expect(writeTextFile).toHaveBeenCalledOnce()); expect(vi.mocked(writeTextFile).mock.calls[0][1]).toContain("not adopted"); });
  it("does not write if bundle reauthorization fails or changes", async () => { const {save} = await import("@tauri-apps/plugin-dialog"); const {writeTextFile} = await import("@tauri-apps/plugin-fs"); vi.mocked(save).mockImplementation(async () => { vi.mocked(getPageExportBundle).mockRejectedValue(new Error("access revoked")); return "/tmp/page.md"; }); wrap(); await waitFor(() => expect(screen.getByRole("button",{name:"Export"})).toBeEnabled()); fireEvent.click(screen.getByRole("button",{name:"Export"})); expect(await screen.findByRole("alert")).toHaveTextContent("access revoked"); expect(writeTextFile).not.toHaveBeenCalled(); });
  it("waits for a new capture and its own version list when reopened", async () => {
    const client = new QueryClient({defaultOptions:{queries:{retry:false}}});
    const element = <QueryClientProvider client={client}><PageExportDialog pageId="page-1" onClose={vi.fn()} /></QueryClientProvider>;
    const view = render(element); await waitFor(() => expect(screen.getByRole("button", {name:"Export"})).toBeEnabled()); view.unmount();
    let finish!: (value: PageExportBundle) => void;
    const updated = {...base,id:"updated",digest:"updated-digest",version:5,content:"Changed current"};
    vi.mocked(capturePageExport).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    vi.mocked(listPageExportVersions).mockResolvedValue({versions:[version(updated),version(old)]});
    vi.mocked(getPageExportBundle).mockImplementation(async (_page,id) => id === "updated" ? updated : old);
    const previousLists = vi.mocked(listPageExportVersions).mock.calls.length;
    render(element); expect(screen.getByRole("button",{name:"Export"})).toBeDisabled(); expect(vi.mocked(listPageExportVersions)).toHaveBeenCalledTimes(previousLists);
    finish(updated);
    await waitFor(() => expect(screen.getByLabelText("Version")).toHaveValue("updated"));
    await waitFor(() => expect(screen.getByRole("button",{name:"Export"})).toBeEnabled());
    expect(vi.mocked(listPageExportVersions)).toHaveBeenCalledTimes(previousLists + 1);
  });
  it("disables cached bundle data when its fresh authorization fails", async () => {
    const client = new QueryClient({defaultOptions:{queries:{retry:false}}});
    const element = <QueryClientProvider client={client}><PageExportDialog pageId="page-1" onClose={vi.fn()} /></QueryClientProvider>;
    const view = render(element); await waitFor(() => expect(screen.getByRole("button",{name:"Export"})).toBeEnabled()); view.unmount();
    vi.mocked(getPageExportBundle).mockRejectedValue(new Error("access revoked"));
    render(element); expect(await screen.findByRole("alert")).toHaveTextContent("Could not load");
    expect(screen.getByRole("button",{name:"Export"})).toBeDisabled();
  });
});
