// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { useState } from "react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { publishPageDraft, type Page } from "../../../lib/tauri";
import { PageProjectionNotice } from "./PageProjectionNotice";
vi.mock("../../../lib/tauri", () => ({ publishPageDraft: vi.fn() }));
const result = (extra: Partial<Page>) => ({ id:"page",version:2,status:"active",title:"Saved page",content:"Saved content",...extra }) as Page;
function setup(disabled=false) { const onResolved=vi.fn();render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><PageProjectionNotice pageId="page" issue={{expectedVersion:1}} disabled={disabled} onResolved={onResolved}/></QueryClientProvider>);return {onResolved}; }
beforeEach(()=>{vi.mocked(publishPageDraft).mockReset();});
describe("actual pending file save",()=>{
 it("keeps saved-note wording and retries the same published version without private error text",async()=>{vi.mocked(publishPageDraft).mockResolvedValueOnce(result({projection_status:"pending",projection_error:"Private /Users/example/secret/folder unavailable"})).mockResolvedValueOnce(result({projection_status:"synced",storage_path:"Work/note.md"}));const {onResolved}=setup();expect(screen.getByRole("status")).toHaveTextContent("Your page is saved");const user=userEvent.setup();await user.click(screen.getByRole("button",{name:"Retry saving the file"}));expect(await screen.findByText("The file still could not be saved. Check that the folder is available, then retry.")).toBeInTheDocument();expect(screen.queryByText(/Users\/example/)).not.toBeInTheDocument();await user.click(screen.getByRole("button",{name:"Retry saving the file"}));expect(publishPageDraft).toHaveBeenNthCalledWith(1,{id:"page",expectedVersion:1});expect(publishPageDraft).toHaveBeenNthCalledWith(2,{id:"page",expectedVersion:1});expect(onResolved).toHaveBeenCalledOnce();expect(screen.queryByRole("status")).not.toBeInTheDocument();});
 it("keeps page access and notice on stale-version refusal",async()=>{vi.mocked(publishPageDraft).mockRejectedValue(new Error("draft_version_conflict"));setup();await userEvent.setup().click(screen.getByRole("button",{name:"Retry saving the file"}));expect(await screen.findByRole("status")).toHaveTextContent("The file still could not be saved");});
 it("does not reopen A or leak resolved state into B after a deferred retry",async()=>{
  let resolve!: (page:Page)=>void;
  vi.mocked(publishPageDraft).mockReturnValue(new Promise<Page>(done=>{resolve=done;}));
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  function Harness(){const [view,setView]=useState("A");return <QueryClientProvider client={client}><p>Current {view}</p><button onClick={()=>setView("B")}>Navigate B</button><PageProjectionNotice key={view} pageId={view} issue={{expectedVersion:1}} disabled={false} onResolved={()=>setView(view)}/></QueryClientProvider>;}
  render(<Harness/>);
  const user=userEvent.setup();await user.click(screen.getByRole("button",{name:"Retry saving the file"}));
  await user.click(screen.getByRole("button",{name:"Navigate B"}));
  await act(async()=>resolve(result({id:"A",projection_status:"synced",storage_path:"Work/A.md"})));
  expect(screen.getByText("Current B")).toBeInTheDocument();
  expect(screen.getByRole("button",{name:"Retry saving the file"})).toBeEnabled();
  expect(publishPageDraft).toHaveBeenCalledOnce();
 });
 it("blocks retry during an unsaved edit",()=>{setup(true);expect(screen.getByRole("button",{name:"Retry saving the file"})).toBeDisabled();expect(publishPageDraft).not.toHaveBeenCalled();});
});
