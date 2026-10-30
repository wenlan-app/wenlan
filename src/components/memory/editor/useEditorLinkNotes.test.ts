import {createElement,type PropsWithChildren} from "react";
import {QueryClient,QueryClientProvider} from "@tanstack/react-query";
import {renderHook,waitFor} from "@testing-library/react";
import {beforeEach,expect,it,vi} from "vitest";
import {loadAllLinkCandidates} from "../page/linkCandidates";
import {useEditorLinkNotes} from "./useEditorLinkNotes";
import type {Page} from "../../../lib/tauri";
vi.mock("../page/linkCandidates",async original=>({...await original<typeof import("../page/linkCandidates")>(),loadAllLinkCandidates:vi.fn()}));
const load=vi.mocked(loadAllLinkCandidates);
const wrapper=({children}:PropsWithChildren)=>createElement(QueryClientProvider,{client:new QueryClient({defaultOptions:{queries:{retry:false}}})},children);
beforeEach(()=>{load.mockReset();});
it.each([null,"unfiled","Work"])("loads existing scoped candidates on demand for %s and offers only connected actions",async space=>{
  load.mockResolvedValue([{name:space,pages:[{id:"target",title:"Target",status:"active",space,creation_kind:"authored"} as Page]}]);
  const {result,rerender}=renderHook(({requested})=>useEditorLinkNotes(space,"self",requested),{initialProps:{requested:false},wrapper});
  expect(load).not.toHaveBeenCalled();rerender({requested:true});
  await waitFor(()=>expect(result.current).toHaveLength(1));
  expect(load).toHaveBeenCalledExactlyOnceWith(space);
  expect(result.current[0]).toMatchObject({id:"target",canUseAsSource:false});
});
