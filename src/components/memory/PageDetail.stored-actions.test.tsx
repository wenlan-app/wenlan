// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";
import { setConfirmPresenter, type ConfirmActionLabels } from "../../lib/confirmAction";
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
  restorePage: vi.fn(),
  clipboardWrite: vi.fn(),
  exportPageToObsidian: vi.fn(),
  getTruthStatus: vi.fn(),
  pageReviewSupported: vi.fn(),
  reviewPage: vi.fn(),
  parseEntityGuardError: vi.fn().mockReturnValue(null),
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn(),
  getResolvedRouting: vi.fn(),
}));

vi.mock("./PageCanvas", () => ({ default: () => <div /> }));

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({entities:[],relations:[],memories:[],memory_links:[],pages:[],page_links:[]}),
  // Fails closed, which is what an older or unreachable daemon looks like:
  // the review action stays disabled unless a test opts in.
  ...tauriMocks,
  RestorePageApiError: class RestorePageApiError extends Error {},
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
  tauriMocks.listPendingRevisions.mockResolvedValue([]); tauriMocks.listRefinements.mockResolvedValue({proposals:[]});
  tauriMocks.getResolvedRouting.mockResolvedValue(null);
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
 it('archive flushes the latest body, keeps the tab open read only and never runs the delete path',async()=>{
  tauriMocks.archivePage.mockImplementation(async()=>{current={...current,status:'archived',version:current.version+1};});
  const onBack=vi.fn(),onDeleted=vi.fn();const {user,editor,client}=await start({onBack,onDeleted});
  const invalidation=vi.spyOn(client,'invalidateQueries');
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Keep this archived body'));
  await user.click(await action(user,'Archive page'));
  expect(await screen.findByText('This page is archived. Restore it to edit.')).toBeVisible();
  expect(tauriMocks.archivePage).toHaveBeenCalledWith('page-1');
  expect(tauriMocks.updatePage.mock.invocationCallOrder[0]).toBeLessThan(tauriMocks.archivePage.mock.invocationCallOrder[0]);
  expect(current.content).toBe('Keep this archived body');
  expect(tauriMocks.deletePage).not.toHaveBeenCalled();
  expect(onDeleted).not.toHaveBeenCalled();
  expect(onBack).not.toHaveBeenCalled();
  expect(window.confirm).toHaveBeenCalledWith('Archive this page? You can restore it from Archived pages.');
  expect(invalidation).toHaveBeenCalledWith({queryKey:['pages']});
  await waitFor(()=>expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull());
  await user.click(screen.getByRole('button',{name:'Page actions'}));
  expect(screen.queryByRole('menuitem',{name:'Edit page'})).toBeNull();
  expect(screen.queryByRole('menuitem',{name:'Archive page'})).toBeNull();
  expect(screen.getByRole('menuitem',{name:'Delete page'})).toBeInTheDocument();
 });
 it('an archived page offers Restore from its banner and is editable again after restore',async()=>{
  Object.defineProperty(HTMLDialogElement.prototype,'showModal',{configurable:true,value:function(this:HTMLDialogElement){this.open=true;}});
  Object.defineProperty(HTMLDialogElement.prototype,'close',{configurable:true,value:function(this:HTMLDialogElement){this.open=false;}});
  current={...current,status:'archived',version:4};
  tauriMocks.restorePage.mockImplementation(async(_id:string,expectedVersion:number)=>{current={...current,status:'active',version:expectedVersion+1};return{id:'page-1',title:current.title,version:current.version,status:'active',projection_status:'synced'};});
  const onRestored=vi.fn();
  const {user}=renderWithQuery(<PageDetail pageId="page-1" initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onRestored={onRestored}/>);
  await screen.findByText('This page is archived. Restore it to edit.');
  expect(screen.queryByRole('textbox',{name:'Page editor'})).toBeNull();
  await user.click(screen.getByRole('button',{name:'Restore page'}));
  const dialog=await screen.findByRole('dialog');
  await user.click(within(dialog).getByRole('button',{name:'Restore page'}));
  await waitFor(()=>expect(tauriMocks.restorePage).toHaveBeenCalledWith('page-1',4,'Link Test Page'));
  await waitFor(()=>expect(screen.queryByText('This page is archived. Restore it to edit.')).toBeNull());
  expect(onRestored).toHaveBeenCalledWith('page-1');
  // The tab asked for editing; once the page is active again it gets it.
  expect(await screen.findByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('an archived page hides its pending review line and shows it again after restore',async()=>{
  Object.defineProperty(HTMLDialogElement.prototype,'showModal',{configurable:true,value:function(this:HTMLDialogElement){this.open=true;}});
  Object.defineProperty(HTMLDialogElement.prototype,'close',{configurable:true,value:function(this:HTMLDialogElement){this.open=false;}});
  tauriMocks.listPendingRevisions.mockResolvedValue([{target_source_id:'page-1',revision_source_id:'revision-page-1',revision_content:'proposal',source_agent:'codex',last_modified:1,target_kind:'page'}]);
  current={...current,status:'archived',version:4};
  tauriMocks.restorePage.mockImplementation(async(_id:string,expectedVersion:number)=>{current={...current,status:'active',version:expectedVersion+1};return{id:'page-1',title:current.title,version:current.version,status:'active',projection_status:'synced'};});
  const onOpenReview=vi.fn();
  const {user}=renderWithQuery(<PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} onOpenReview={onOpenReview}/>);
  await screen.findByText('This page is archived. Restore it to edit.');
  // Give the review queries a chance to answer: the line must still stay hidden.
  await waitFor(()=>expect(tauriMocks.getPage).toHaveBeenCalled());
  await act(async()=>{await new Promise(resolve=>setTimeout(resolve,50));});
  expect(screen.queryByText('1 item needs review')).toBeNull();
  expect(screen.queryByRole('button',{name:'Review changes'})).toBeNull();
  await user.click(screen.getByRole('button',{name:'Restore page'}));
  const dialog=await screen.findByRole('dialog');
  await user.click(within(dialog).getByRole('button',{name:'Restore page'}));
  await waitFor(()=>expect(screen.queryByText('This page is archived. Restore it to edit.')).toBeNull());
  expect(await screen.findByText('1 item needs review')).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Review changes'}));
  expect(onOpenReview).toHaveBeenCalledWith('revision:page-1');
 });
 it('an active page shows its pending review line',async()=>{
  tauriMocks.listPendingRevisions.mockResolvedValue([{target_source_id:'page-1',revision_source_id:'revision-page-1',revision_content:'proposal',source_agent:'codex',last_modified:1,target_kind:'page'}]);
  renderWithQuery(<PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} onOpenReview={vi.fn()}/>);
  expect(await screen.findByText('1 item needs review')).toBeVisible();
  expect(screen.queryByText('This page is archived. Restore it to edit.')).toBeNull();
 });
 it('Archive asks with a neutral confirm button while Delete page stays red',async()=>{
  const asked:ConfirmActionLabels[]=[];
  setConfirmPresenter(async(_message,labels)=>{asked.push(labels);return false;});
  try {
   const {user}=await start();
   await user.click(await action(user,'Archive page'));
   await waitFor(()=>expect(asked).toHaveLength(1));
   expect(asked[0]).toMatchObject({confirm:'Archive page',tone:'neutral'});
   await user.click(await action(user,'Delete page'));
   await waitFor(()=>expect(asked).toHaveLength(2));
   expect(asked[1].confirm).toBe('Delete page');
   expect(asked[1].tone ?? 'danger').toBe('danger');
   expect(tauriMocks.archivePage).not.toHaveBeenCalled();expect(tauriMocks.deletePage).not.toHaveBeenCalled();
  } finally { setConfirmPresenter((message)=>Promise.resolve(window.confirm(message))); }
 });
 it('autosave never writes an archived page, but a cache miss is left to the daemon',async()=>{
  const {editor,client}=await start();
  client.removeQueries({queryKey:['page','page-1'],exact:true});
  act(()=>replaceDocument(editorViewFromTextbox(editor),'Written after a cache miss'));
  await waitFor(()=>expect(tauriMocks.updatePage).toHaveBeenCalled());
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
 it('archive cancellation keeps the saved page open',async()=>{
  vi.mocked(window.confirm).mockReturnValue(false);
  const onBack=vi.fn();const {user}=await start({onBack});
  await user.click(await action(user,'Archive page'));
  await waitFor(()=>expect(window.confirm).toHaveBeenCalled());
  expect(tauriMocks.archivePage).not.toHaveBeenCalled();expect(onBack).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('archive errors keep the page and show the archive-specific message',async()=>{
  tauriMocks.archivePage.mockRejectedValue(new Error('offline'));
  const onBack=vi.fn();const {user}=await start({onBack});
  await user.click(await action(user,'Archive page'));
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not archive this page. Try again.');
  expect(onBack).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox',{name:'Page editor'})).toBeVisible();
 });
 it('directory archive uses the same composition guard and pending editor lock',async()=>{
  let archive:(()=>Promise<boolean>)|null=null;
  let flush:(()=>Promise<boolean>)|null=null;
  let release!:()=>void;
  tauriMocks.archivePage.mockImplementation(()=>new Promise<void>(resolve=>{release=resolve;}));
  const {editor}=await start({onRegisterArchive:(_id,fn)=>{archive=fn;},onRegisterFlush:fn=>{flush=fn;}});
  fireEvent.compositionStart(editor,{data:''});
  await act(async()=>{expect(await archive!()).toBe(false);});
  expect(tauriMocks.archivePage).not.toHaveBeenCalled();
  fireEvent.compositionEnd(editor,{data:''});
  await waitFor(()=>expect(editorViewFromTextbox(editor).composing).toBe(false));
  let pending!:Promise<boolean>;
  await act(async()=>{pending=archive!();});
  // Archive first waits for the editor to settle after the composition; the
  // default 1 s wait has too little room for that on a busy CI runner.
  await waitFor(()=>expect(tauriMocks.archivePage).toHaveBeenCalledOnce(),{timeout:5_000});
  expect(await flush!()).toBe(false);
  await waitFor(()=>expect(editor).toHaveAttribute('contenteditable','false'));
  await act(async()=>{expect(await archive!()).toBe(false);});
  expect(tauriMocks.archivePage).toHaveBeenCalledOnce();
  await act(async()=>{release();expect(await pending).toBe(true);});
 });
 it('archived pages cannot be archived or edited through a registered directory action',async()=>{
  current={...current,status:'archived'};
  let archive:(()=>Promise<boolean>)|null=null;
  const {user}=renderWithQuery(<PageDetail pageId="page-1" initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onRegisterArchive={(_id,fn)=>{archive=fn;}}/>);
  await screen.findByText('Link Test Page');
  await user.click(screen.getByRole('button',{name:'Page actions'}));
  expect(screen.queryByRole('menuitem',{name:'Archive page'})).not.toBeInTheDocument();
  await act(async()=>{expect(await archive!()).toBe(false);});
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
