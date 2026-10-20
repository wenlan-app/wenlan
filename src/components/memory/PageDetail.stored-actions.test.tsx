// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
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
