import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getPage, getZoteroDisconnectPreview, removeSource, type Page, type ZoteroDisconnectPreview } from "../../../lib/tauri";
import ZoteroDisconnectDialog from "./ZoteroDisconnectDialog";
vi.mock("../../../lib/tauri", async original => ({ ...(await original<typeof import("../../../lib/tauri")>()), getPage:vi.fn(),getZoteroDisconnectPreview:vi.fn(),removeSource:vi.fn() }));
const preview: ZoteroDisconnectPreview = {identity:{registration_id:"registration",locator:"secret-path",server_id:"server",space_id:"space-id"},identity_digest:"digest",space_name:"Research",revision:2,canonical_item_ids:["private-paper"],affected_page_ids:["own-id","foreign-id","missing-id"],impact_scope_known:true,pending_disconnect:false,disconnected:false};
function show(onClose=vi.fn(), client=new QueryClient({defaultOptions:{queries:{retry:false}}})) { return {onClose,client,...render(<QueryClientProvider client={client}><ZoteroDisconnectDialog id="registration" onClose={onClose} /></QueryClientProvider>)}; }
beforeEach(() => { vi.resetAllMocks(); vi.mocked(getZoteroDisconnectPreview).mockResolvedValue(preview); vi.mocked(getPage).mockImplementation(async id => {if(id === "missing-id") throw new Error("gone"); return {id,space:id === "own-id" ? "Research" : "Other",title:id === "own-id" ? "Evidence review" : "Private title"} as Page;}); });
describe("Zotero disconnect confirmation", () => {
  it("explains unknown legacy impact without false counts and still permits disconnect", async () => {
    const legacy = {...preview, impact_scope_known:false, canonical_item_ids:[], affected_page_ids:[]};
    vi.mocked(getZoteroDisconnectPreview).mockResolvedValue(legacy); show();
    expect(await screen.findByText("This older connection cannot identify affected pages. Saved PDFs, pages and tags will be kept.")).toBeInTheDocument();
    expect(screen.queryByText(/Retained papers:/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Affected research pages:/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Disconnect"}));
    await waitFor(() => expect(removeSource).toHaveBeenCalledWith("registration",legacy));
    expect(getPage).not.toHaveBeenCalled();
  });
  it("requires fresh impact, shows same-Space titles and sends the full fenced preview", async () => {
    let finish!: (value:ZoteroDisconnectPreview)=>void; vi.mocked(getZoteroDisconnectPreview).mockImplementation(() => new Promise(resolve => {finish=resolve;}));
    const {onClose}=show(); expect(screen.getByRole("button",{name:"Disconnect"})).toBeDisabled(); expect(removeSource).not.toHaveBeenCalled(); finish(preview);
    expect(await screen.findByText("Evidence review")).toBeInTheDocument(); await waitFor(() => expect(screen.getAllByText("Page unavailable")).toHaveLength(2));
    expect(screen.getByText("Retained papers: 1")).toBeInTheDocument(); expect(screen.getByText("Affected research pages: 3")).toBeInTheDocument();
    for (const hidden of ["own-id","foreign-id","missing-id","Private title","secret-path","private-paper"]) expect(screen.queryByText(hidden)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Disconnect"})); await waitFor(() => expect(removeSource).toHaveBeenCalledWith("registration",preview)); await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });
  it("refuses failed preview and retries it before permitting disconnect", async () => {
    vi.mocked(getZoteroDisconnectPreview).mockRejectedValueOnce(new Error("offline")).mockResolvedValue(preview); show();
    expect(await screen.findByText(/Could not check the impact/)).toBeInTheDocument(); expect(screen.getByRole("button",{name:"Disconnect"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button",{name:"Try again"})); await waitFor(() => expect(screen.getByRole("button",{name:"Disconnect"})).toBeEnabled()); expect(removeSource).not.toHaveBeenCalled();
  });
  it("refreshes changed preview after a failed mutation and requires another click", async () => {
    const updated = {...preview, revision:3,identity_digest:"updated",pending_disconnect:true};
    vi.mocked(removeSource).mockRejectedValueOnce(new Error("conflict")).mockResolvedValueOnce(); vi.mocked(getZoteroDisconnectPreview).mockResolvedValueOnce(preview).mockResolvedValue(updated);
    const {onClose}=show(); await waitFor(() => expect(screen.getByRole("button",{name:"Disconnect"})).toBeEnabled()); fireEvent.click(screen.getByRole("button",{name:"Disconnect"}));
    expect(await screen.findByText(/previous disconnect was interrupted/)).toBeInTheDocument(); expect(onClose).not.toHaveBeenCalled(); expect(removeSource).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button",{name:"Disconnect"})); await waitFor(() => expect(removeSource).toHaveBeenLastCalledWith("registration",updated));
  });
  it("does not close on cancel or Escape while the mutation is pending", async () => {
    vi.mocked(removeSource).mockImplementation(() => new Promise(() => {})); const {onClose}=show(); await waitFor(() => expect(screen.getByRole("button",{name:"Disconnect"})).toBeEnabled());
    fireEvent.click(screen.getByRole("button",{name:"Disconnect"})); await screen.findByRole("button",{name:"Disconnecting…"}); fireEvent.keyDown(document,{key:"Escape"}); fireEvent.click(screen.getByRole("button",{name:"Close"})); expect(onClose).not.toHaveBeenCalled();
  });
  it("requires a fresh preview when reopened with the same query client", async () => {
    const client = new QueryClient({defaultOptions:{queries:{retry:false}}});
    const first=show(vi.fn(),client); await waitFor(() => expect(screen.getByRole("button",{name:"Disconnect"})).toBeEnabled()); first.unmount();
    let finish!: (value:ZoteroDisconnectPreview)=>void;
    vi.mocked(getZoteroDisconnectPreview).mockImplementation(() => new Promise(resolve => {finish=resolve;}));
    show(vi.fn(),client); expect(screen.getByRole("button",{name:"Disconnect"})).toBeDisabled();
    const next={...preview,revision:9,identity_digest:"new-fence"}; finish(next);
    await waitFor(() => expect(screen.getByRole("button",{name:"Disconnect"})).toBeEnabled()); fireEvent.click(screen.getByRole("button",{name:"Disconnect"}));
    await waitFor(() => expect(removeSource).toHaveBeenCalledWith("registration",next));
  });
});
