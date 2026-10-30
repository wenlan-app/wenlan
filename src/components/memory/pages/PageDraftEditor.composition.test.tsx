import {act,fireEvent,render,screen} from "@testing-library/react";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {createRef} from "react";
import {afterEach,expect,it,vi} from "vitest";
import {createPageDraft,publishPageDraft,type Page} from "../../../lib/tauri";
import {PageDraftEditor,type PageDraftEditorHandle} from "./PageDraftEditor";
vi.mock("../../../lib/tauri",()=>({createPageDraft:vi.fn(),discardPageDraft:vi.fn(),getPage:vi.fn(),publishPageDraft:vi.fn(),updatePageDraft:vi.fn()}));
vi.mock("../editor/useEditorLinkNotes",()=>({useEditorLinkNotes:()=>[]}));
const controls=vi.hoisted(()=>({change:null as null|((source:string)=>void)}));
vi.mock("../editor/MarkdownEditor",()=>({MarkdownEditor:(props:any)=>{controls.change=props.onDocumentChange;return <button onClick={()=>props.onStatusChange({compositionActive:false})}>Body ready</button>;}}));
afterEach(()=>vi.useRealTimers());
it("does not let a lazy body-ready status release an active title composition",async()=>{
  vi.useFakeTimers();const ref=createRef<PageDraftEditorHandle>();
  render(<QueryClientProvider client={new QueryClient()}><PageDraftEditor ref={ref} space={null} onBack={vi.fn()} onOpenExisting={vi.fn()} onPublished={vi.fn()}/></QueryClientProvider>);
  const title=screen.getByRole("textbox",{name:"Title"});
  fireEvent.compositionStart(title);fireEvent.change(title,{target:{value:"中文標題"}});fireEvent.click(screen.getByRole("button",{name:"Body ready"}));
  await act(async()=>{await vi.advanceTimersByTimeAsync(1000);expect(await ref.current?.flush()).toBe(false);});
  expect(createPageDraft).not.toHaveBeenCalled();
});

it("flushes same-batch editor input before the parent can render it",async()=>{
  let latest:Page;
  vi.mocked(createPageDraft).mockImplementation(async input=>latest={id:input.clientDraftId,title:input.title,content:input.content,space:input.space,version:1,status:"draft",creation_kind:"authored"} as Page);
  vi.mocked(publishPageDraft).mockImplementation(async()=>({...latest,status:"active",version:2}));
  const ref=createRef<PageDraftEditorHandle>();
  render(<QueryClientProvider client={new QueryClient()}><PageDraftEditor ref={ref} space={null} onBack={vi.fn()} onOpenExisting={vi.fn()} onPublished={vi.fn()}/></QueryClientProvider>);
  await act(async()=>{controls.change?.("The last same-event keystroke.");expect(await ref.current?.flush()).toBe(true);});
  expect(createPageDraft).toHaveBeenCalledWith(expect.objectContaining({content:"The last same-event keystroke."}));
  expect(publishPageDraft).toHaveBeenCalledTimes(1);
});
