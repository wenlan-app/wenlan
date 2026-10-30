// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { setExampleWikiActive } from "../../lib/exampleWiki/mode";
import { i18n } from "../../i18n";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  replaceDocument,
} from "./editor/editorTestUtils";

const tauriMocks = vi.hoisted(() => ({
  getPage: vi.fn(),
  getPageSources: vi.fn(),
  listRegisteredSources: vi.fn(),
  getPageLinks: vi.fn(),
  listOrphanLinks: vi.fn(),
  getPageRevisions: vi.fn(),
  listPages: vi.fn(),
  redistillPage: vi.fn(),
  updatePage: vi.fn(),
  getDaemonVersion: vi.fn(),
  getSystemInfo: vi.fn(),
  daemonMeetsFloor: vi.fn(),
  recordPageEditorDiagnostic: vi.fn(),
  deletePage: vi.fn(),
  archivePage: vi.fn(),
  clipboardWrite: vi.fn(),
  exportPageToObsidian: vi.fn(),
  getTruthStatus: vi.fn(),
  pageReviewSupported: vi.fn(),
  reviewPage: vi.fn(),
  parseEntityGuardError: vi.fn().mockReturnValue(null),
}));

vi.mock("./PageCanvas", () => ({ default: () => <div /> }));

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({entities:[],relations:[],memories:[],memory_links:[],pages:[],page_links:[]}),
  // Fails closed, which is what an older or unreachable daemon looks like:
  // the review action stays disabled unless a test opts in.
  ...tauriMocks,
  FACET_COLORS: {},
  STABILITY_TIERS: {},
}));

const LINKED_PAGE = {
  id: "page-1",
  title: "Link Test Page",
  summary: null,
  content:
    "Intro sentence.\n\nThis page references [[Resolved Link]] and [[Missing Link]].\n\nIt also cites [memory](#memory:mem-1).",
  entity_id: null,
  domain: "testing",
  source_memory_ids: ["mem-1"],
  version: 1,
  status: "active",
  created_at: "2026-06-26T00:00:00+00:00",
  last_compiled: "2026-06-26T00:00:00+00:00",
  last_modified: "2026-06-26T00:00:00+00:00",
};

function renderWithQuery(ui: React.ReactElement, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return {
    client,
    user: userEvent.setup(),
    ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>),
  };
}

beforeAll(() => {
  installCodeMirrorDomPolyfills();
});

describe("PageDetail stored actions in seamless editing", () => {
 let current = { ...LINKED_PAGE, user_edited: false };
 beforeEach(() => {
  vi.clearAllMocks(); current = { ...LINKED_PAGE, user_edited: false };
  tauriMocks.getPage.mockImplementation(async () => current);
  tauriMocks.getPageSources.mockResolvedValue([{source:{page_id:'page-1',memory_source_id:'mem-1',linked_at:0},memory:null}]); tauriMocks.listRegisteredSources.mockResolvedValue([]);
  tauriMocks.getPageLinks.mockResolvedValue({outbound:[],inbound:[]});
  tauriMocks.listOrphanLinks.mockResolvedValue({min_count:2,orphan_labels:[]});
  tauriMocks.getPageRevisions.mockResolvedValue({page_id:'page-1',current_version:1,user_edited:false,entries:[]});
  tauriMocks.listPages.mockResolvedValue([]); tauriMocks.getDaemonVersion.mockResolvedValue('0.14.1');
  tauriMocks.getSystemInfo.mockResolvedValue({os:'macos'}); tauriMocks.daemonMeetsFloor.mockReturnValue(true);
  tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined); tauriMocks.getTruthStatus.mockResolvedValue(null);
  tauriMocks.pageReviewSupported.mockResolvedValue('ready'); tauriMocks.reviewPage.mockResolvedValue({kind:'applied'});
  tauriMocks.archivePage.mockReset().mockResolvedValue(undefined);
  tauriMocks.deletePage.mockResolvedValue(undefined); tauriMocks.redistillPage.mockResolvedValue({status:'ok',updated:true});
  tauriMocks.updatePage.mockImplementation(async input => {current={...current,content:input.content,version:input.expectedVersion+1,user_edited:true};return {outcome:'saved'};});
  vi.spyOn(window,'confirm').mockReturnValue(true);
 });
 async function start(extra:Partial<React.ComponentProps<typeof PageDetail>>={}) {
  const r=renderWithQuery(<PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} initialMode="edit" {...extra}/>);
  const editor=await screen.findByRole('textbox',{name:'Page editor'});
  return {...r,editor};
 }
 async function action(user:ReturnType<typeof userEvent.setup>,label:string) {
  await user.click(screen.getByRole('button',{name:i18n.t('pageDetail.actions')}));
  return screen.getByRole('menuitem',{name:label});
 }
 async function reviewAction(user:ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button',{name:i18n.t('pageInspector.open')}));
  return screen.getByRole('button',{name:i18n.t('pageDetail.markPageReviewed')});
 }
 it('archived arrival while the asynchronous editor opener waits cannot open its stale active source', async () => {
  let finish!: (version: string) => void;
  tauriMocks.getDaemonVersion.mockReturnValueOnce(new Promise<string>(resolve => { finish = resolve; }));
  const {client}=renderWithQuery(<PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} initialMode="edit"/>);
  await waitFor(()=>expect(tauriMocks.getDaemonVersion).toHaveBeenCalled());
  current={...current,status:'archived',version:2};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(screen.getByText(i18n.t('pages.archive.readOnly'))).toBeVisible());
  await act(async()=>finish('0.14.1'));
  expect(document.querySelector('.page-editor-stack')).toBeNull();
  expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull();
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();
 });
 it('archived background arrival locks a clean mounted editor and restore uses the new version',async()=>{
  let flush:(()=>Promise<boolean>)|null=null;
  const {editor,client}=await start({onRegisterFlush:value=>{flush=value;}});
  current={...current,status:'archived',version:2};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','false'));
  expect(editorViewFromTextbox(editor).state.doc.toString()).toBe(current.content);
  await act(async()=>expect(await flush!()).toBe(true));
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  current={...current,status:'active',version:3};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','true'));
  act(()=>replaceDocument(editorViewFromTextbox(editor),'After real restore'));
  await act(async()=>expect(await flush!()).toBe(true));
  expect(tauriMocks.updatePage).toHaveBeenCalledWith(expect.objectContaining({expectedVersion:3,content:'After real restore'}));
 });
 it('archived background arrival preserves dirty local text and blocks save and navigation until recovery',async()=>{
  let flush:(()=>Promise<boolean>)|null=null;
  const onBack=vi.fn();const {editor,client,user}=await start({onBack,onRegisterFlush:value=>{flush=value;}});
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Unsaved local recovery text'));
  current={...current,status:'archived',version:2};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','false'));
  expect(editorViewFromTextbox(editor).state.doc.toString()).toBe('Unsaved local recovery text');
  await act(async()=>expect(await flush!()).toBe(false));
  await user.click(screen.getByRole('button',{name:'Copy my draft'}));
  expect(tauriMocks.clipboardWrite).toHaveBeenCalledWith('Unsaved local recovery text');
  await waitFor(()=>expect(screen.getByRole('button',{name:'Reload latest'})).toBeEnabled());
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();expect(onBack).not.toHaveBeenCalled();
  current={...current,status:'active',version:3};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(screen.getByRole('button',{name:'Reload latest'})).toBeEnabled());
  expect(editorViewFromTextbox(editor).state.doc.toString()).toBe('Unsaved local recovery text');
  await act(async()=>expect(await flush!()).toBe(false));
  await user.click(screen.getByRole('button',{name:'Reload latest'}));
  await waitFor(()=>expect(editorViewFromTextbox(screen.getByRole('textbox',{name:'Page editor'})).state.doc.toString()).toBe(current.content));
  const recovered=screen.getByRole('textbox',{name:'Page editor'});
  act(()=>replaceDocument(editorViewFromTextbox(recovered),'Unsaved local recovery text'));
  await act(async()=>expect(await flush!()).toBe(true));
  expect(tauriMocks.updatePage).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({expectedVersion:3,content:'Unsaved local recovery text'}));
 });
 it('archived clean background content shows the newest readonly body',async()=>{
  const {client}=await start();
  current={...current,status:'archived',version:3,content:'Newest archived canonical body'};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull());
  expect(await screen.findByText('Newest archived canonical body')).toBeVisible();
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();
 });
 it('archived changed body does not discard a clean live preedit session',async()=>{
  const {editor,client}=await start();
  fireEvent.compositionStart(editor,{data:''});
  current={...current,status:'archived',version:3,content:'New canonical while composing'};
  act(()=>client.setQueryData(['page','page-1'],current));
  await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','false'));
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBe(editor);
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();
 });
 it('archived dirty recovery can cancel or confirm reload into readonly without restoring or writing',async()=>{
  let flush:(()=>Promise<boolean>)|null=null;
  const {editor,client,user}=await start({onRegisterFlush:value=>{flush=value;}});
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Local draft to copy before leaving'));
  current={...current,status:'archived',version:2,content:'Archived canonical body'};
  act(()=>client.setQueryData(['page','page-1'],current));
  const reload=await screen.findByRole('button',{name:'Reload latest'});
  await waitFor(()=>expect(reload).toBeEnabled());
  vi.mocked(window.confirm).mockReturnValueOnce(false);
  await user.click(reload);
  expect(editorViewFromTextbox(editor).state.doc.toString()).toBe('Local draft to copy before leaving');
  await act(async()=>expect(await flush!()).toBe(false));
  await user.click(reload);
  await waitFor(()=>expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull());
  expect(await screen.findByText('Archived canonical body')).toBeVisible();
  await act(async()=>expect(await flush!()).toBe(true));
  expect(current.status).toBe('archived');expect(tauriMocks.updatePage).not.toHaveBeenCalled();
 });
 it.each(['clean','dirty','composing'] as const)('archived same-page read intent respects the mounted %s session',async state=>{
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});client.setQueryData(['page','page-1'],current);
  const props={pageId:'page-1',onBack:vi.fn(),onMemoryClick:vi.fn()};
  const ui=(mode:'read'|'edit')=><QueryClientProvider client={client}><PageDetail {...props} initialMode={mode}/></QueryClientProvider>;
  const {rerender}=render(ui('edit'));
  const editor=await screen.findByRole('textbox',{name:'Page editor'});
  if(state==='dirty') act(()=>replaceDocument(editorViewFromTextbox(editor),'Retain local draft'));
  if(state==='composing') fireEvent.compositionStart(editor,{data:''});
  rerender(ui('read'));
  if(state==='clean') expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull();
  else {
    expect(screen.getByRole('textbox',{name:'Page editor'})).toBe(editor);
    if(state==='dirty') expect(editorViewFromTextbox(editor).state.doc.toString()).toBe('Retain local draft');
  }
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();
 });
 it('archived same-page read intent retires a pending automatic opener but explicit edit still works',async()=>{
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});client.setQueryData(['page','page-1'],current);
  let finish!:(version:string)=>void;
  tauriMocks.getDaemonVersion.mockReturnValueOnce(new Promise<string>(resolve=>{finish=resolve;}));
  const props={pageId:'page-1',onBack:vi.fn(),onMemoryClick:vi.fn()};
  const ui=(mode:'read'|'edit')=><QueryClientProvider client={client}><PageDetail {...props} initialMode={mode}/></QueryClientProvider>;
  const {rerender}=render(ui('edit'));
  await waitFor(()=>expect(tauriMocks.getDaemonVersion).toHaveBeenCalled());
  rerender(ui('read'));
  await act(async()=>finish('0.14.1'));
  expect(document.querySelector('.page-editor-stack')).toBeNull();
  const user=userEvent.setup();
  await user.click(screen.getByRole('button',{name:'Page actions'}));
  await user.click(screen.getByRole('menuitem',{name:'Edit page'}));
  expect(await screen.findByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('archived read intent never autoopens a stale active cache before its refetch',async()=>{
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  client.setQueryData(['page','page-1'],current);
  let loaded!:(value:typeof current)=>void;
  tauriMocks.getPage.mockReturnValueOnce(new Promise<typeof current>(resolve=>{loaded=resolve;}));
  renderWithQuery(<PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} initialMode="read"/>,client);
  expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull();
  current={...current,status:'archived',version:2};
  await act(async()=>loaded(current));
  expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull();
  expect(tauriMocks.updatePage).not.toHaveBeenCalled();
 });
 it('example pages keep the shared editor readonly and hide stored mutations',async()=>{
  setExampleWikiActive(true);
  try {
   const {editor}=await start();
   await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','false'));
   expect(screen.queryByRole('button',{name:'Page actions'})).toBeNull();
   expect(screen.queryByRole('button',{name:'Rename page'})).toBeNull();
   expect(tauriMocks.updatePage).not.toHaveBeenCalled();expect(tauriMocks.archivePage).not.toHaveBeenCalled();
  } finally {setExampleWikiActive(false);}
 });
 it('hides Re-distill on a page with no sources to rebuild from',async()=>{
  current={...current,source_memory_ids:[]};
  tauriMocks.getPageSources.mockResolvedValue([]);
  const {user}=await start();
  await user.click(screen.getByRole('button',{name:i18n.t('pageDetail.actions')}));
  expect(screen.getByRole('menuitem',{name:'Delete page'})).toBeInTheDocument();
  expect(screen.queryByRole('menuitem',{name:'Update from sources'})).toBeNull();
 });
 it('exposes existing commands without leaving seamless editing',async()=>{
  const {user}=await start();
  await expect(action(user,'Update from sources')).resolves.toBeEnabled();
  expect(await reviewAction(user)).toBeEnabled();
  expect(await action(user,'Delete page')).toBeEnabled();
 });
 it('flushes edits then reviews exactly the saved body without closing the editor',async()=>{
  const {user,editor}=await start();
  act(()=>replaceDocument(editorViewFromTextbox(editor),'New human content'));
  await user.click(await reviewAction(user));
  await waitFor(()=>expect(tauriMocks.reviewPage).toHaveBeenCalledWith('page-1','New human content'));
  expect(tauriMocks.updatePage.mock.invocationCallOrder[0]).toBeLessThan(tauriMocks.reviewPage.mock.invocationCallOrder[0]);
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('keeps the draft and does not review when saving fails',async()=>{
  tauriMocks.updatePage.mockRejectedValue(new Error('offline'));
  const {user,editor}=await start();act(()=>replaceDocument(editorViewFromTextbox(editor),'Unsaved human content'));
  await user.click(await reviewAction(user));
  await waitFor(()=>expect(tauriMocks.updatePage).toHaveBeenCalled());
  expect(tauriMocks.reviewPage).not.toHaveBeenCalled();
  expect(editorViewFromTextbox(screen.getByRole('textbox',{name:'Page editor'})).state.doc.toString()).toBe('Unsaved human content');
 });
 it('cancelled deletion preserves the saved page and editor',async()=>{
  vi.mocked(window.confirm).mockReturnValue(false);
  const {user,editor}=await start();act(()=>replaceDocument(editorViewFromTextbox(editor),'Keep this content'));
  await user.click(await action(user,'Delete page'));
  await waitFor(()=>expect(window.confirm).toHaveBeenCalled());
  expect(tauriMocks.deletePage).not.toHaveBeenCalled();expect(current.content).toBe('Keep this content');
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('successful deletion navigates only after save and deletion',async()=>{
  const onBack=vi.fn();const {user,editor}=await start({onBack});
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Saved before deletion'));
  await user.click(await action(user,'Delete page'));
  await waitFor(()=>expect(onBack).toHaveBeenCalledOnce());
  expect(tauriMocks.updatePage.mock.invocationCallOrder[0]).toBeLessThan(tauriMocks.deletePage.mock.invocationCallOrder[0]);
 });
 it('archive flushes the latest body and closes the note without permanent deletion',async()=>{
  const onBack=vi.fn(),onDeleted=vi.fn();const {user,editor,client}=await start({onBack,onDeleted});
  const invalidation=vi.spyOn(client,'invalidateQueries');
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Keep this archived body'));
  await user.click(await action(user,'Archive page'));
  await waitFor(()=>expect(onBack).toHaveBeenCalledOnce());
  expect(tauriMocks.archivePage).toHaveBeenCalledWith('page-1');
  expect(tauriMocks.updatePage.mock.invocationCallOrder[0]).toBeLessThan(tauriMocks.archivePage.mock.invocationCallOrder[0]);
  expect(current.content).toBe('Keep this archived body');
  expect(tauriMocks.deletePage).not.toHaveBeenCalled();
  expect(onDeleted).toHaveBeenCalledWith('page-1');
  expect(window.confirm).toHaveBeenCalledWith('Archive this page? You can restore it from Archived pages.');
  expect(invalidation).toHaveBeenCalledWith({queryKey:['pages']});
 });
 it('archive refusal or a failed save preserves the editor and never archives',async()=>{
  tauriMocks.updatePage.mockRejectedValue(new Error('offline'));
  const {user,editor}=await start();
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Unsaved archive body'));
  await user.click(await action(user,'Archive page'));
  await waitFor(()=>expect(tauriMocks.updatePage).toHaveBeenCalled());
  expect(tauriMocks.archivePage).not.toHaveBeenCalled();
  expect(editorViewFromTextbox(screen.getByRole('textbox',{name:'Page editor'})).state.doc.toString()).toBe('Unsaved archive body');
 });
 it('archive cancellation keeps the saved note open',async()=>{
  vi.mocked(window.confirm).mockReturnValue(false);
  const onBack=vi.fn();const {user}=await start({onBack});
  await user.click(await action(user,'Archive page'));
  await waitFor(()=>expect(window.confirm).toHaveBeenCalled());
  expect(tauriMocks.archivePage).not.toHaveBeenCalled();expect(onBack).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('archive errors keep the note and show the archive-specific message',async()=>{
  tauriMocks.archivePage.mockRejectedValue(new Error('offline'));
  const onBack=vi.fn();const {user}=await start({onBack});
  await user.click(await action(user,'Archive page'));
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not archive this page. Try again.');
  expect(onBack).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('directory archive uses the same composition guard and pending editor lock',async()=>{
  let archive:(()=>Promise<void>)|null=null;
  let flush:(()=>Promise<boolean>)|null=null;
  let release!:()=>void;
  tauriMocks.archivePage.mockImplementation(()=>new Promise<void>(resolve=>{release=resolve;}));
  const {editor}=await start({onRegisterArchive:(_id,fn)=>{archive=fn;},onRegisterFlush:fn=>{flush=fn;}});
  fireEvent.compositionStart(editor,{data:''});
  await act(async()=>{await archive!();});
  expect(tauriMocks.archivePage).not.toHaveBeenCalled();
  fireEvent.compositionEnd(editor,{data:''});
  await waitFor(()=>expect(editorViewFromTextbox(editor).composing).toBe(false));
  let pending!:Promise<void>;
  await act(async()=>{pending=archive!();});
  await waitFor(()=>expect(tauriMocks.archivePage).toHaveBeenCalledOnce());
  expect(await flush!()).toBe(false);
  await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','false'));
  await act(async()=>{await archive!();});
  expect(tauriMocks.archivePage).toHaveBeenCalledOnce();
  await act(async()=>{release();await pending;});
 });
 it('archived pages cannot be archived or edited through a registered directory action',async()=>{
  current={...current,status:'archived'};
  let archive:(()=>Promise<void>)|null=null;
  const {user}=renderWithQuery(<PageDetail pageId="page-1" initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onRegisterArchive={(_id,fn)=>{archive=fn;}}/>);
  await screen.findByText('Link Test Page');
  await user.click(screen.getByRole('button',{name:'Page actions'}));
  expect(screen.queryByRole('menuitem',{name:'Archive page'})).not.toBeInTheDocument();
  await act(async()=>{await archive!();});
  expect(tauriMocks.archivePage).not.toHaveBeenCalled();
  expect(screen.queryByRole('textbox',{name:'Page editor'})).not.toBeInTheDocument();
 });
 it('reopens the saved fresh result after rebuilding',async()=>{
  current.user_edited=true;
  tauriMocks.redistillPage.mockImplementation(async()=>{current={...current,content:'Fresh rebuilt content',version:2};return{status:'ok',updated:true};});
  const {user}=await start();
  await user.click(await reviewAction(user));
  await screen.findByText(i18n.t('pageDetail.reviewApplied'));
  await user.click(await action(user,'Update from sources'));
  expect(screen.queryByText(i18n.t('pageDetail.reviewApplied'))).not.toBeInTheDocument();
  await waitFor(()=>expect(tauriMocks.redistillPage).toHaveBeenCalledOnce());
  await waitFor(()=>expect(editorViewFromTextbox(screen.getByRole('textbox',{name:'Page editor'})).state.doc.toString()).toBe('Fresh rebuilt content'));
  expect(window.confirm).toHaveBeenCalled();
 });
 it('does not let pending actions authorize another navigation flush',async()=>{
  let release!:()=>void;tauriMocks.reviewPage.mockImplementation(()=>new Promise(resolve=>{release=()=>resolve({kind:'applied'});}));
  let flush:(()=>Promise<boolean>)|null=null;
  const {user}=await start({onRegisterFlush:fn=>{flush=fn;}});
  await user.click(await reviewAction(user));
  await waitFor(()=>expect(tauriMocks.reviewPage).toHaveBeenCalledOnce());
  expect(await flush!()).toBe(false);
  await act(async()=>release());
  await waitFor(async()=>expect(await flush!()).toBe(true));
 });
});
