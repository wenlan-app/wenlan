import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import KnowledgeContext from "./KnowledgeContext";
import { getKnowledgeGraph } from "../../../lib/tauri";
vi.mock("../../../lib/tauri",()=>({getKnowledgeGraph:vi.fn()}));
function mount() {const open=vi.fn(); render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><KnowledgeContext kind="memory" id="m" title="Saved fact" onNavigatePage={open}/></QueryClientProvider>);return open;}
describe("KnowledgeContext",()=>{
  it("lists pages before a collapsed local graph for a memory",async()=>{
    vi.mocked(getKnowledgeGraph).mockResolvedValue({entities:[],relations:[],memory_links:[],memories:[],pages:[
      {id:"p",title:"Project page",space:null,creation_kind:"source",entity_id:null,last_modified:""},
      {id:"p2",title:"Second page",space:null,creation_kind:"source",entity_id:null,last_modified:""},
    ],page_links:[
      {from:{kind:"page",id:"p"},to:{kind:"memory",id:"m"},link_type:"cites"},
      {from:{kind:"page",id:"p2"},to:{kind:"memory",id:"m"},link_type:"cites"},
    ]});
    const open=mount();
    const section=await screen.findByRole("region",{name:"Used as a source in"});
    const graphDisclosure=screen.getByText("Connections",{selector:"summary"}).closest("details");
    expect(graphDisclosure).not.toHaveAttribute("open");
    expect(section.compareDocumentPosition(graphDisclosure!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await userEvent.click(within(section).getByRole("button",{name:"Project page"}));
    await userEvent.click(screen.getByText("Connections",{selector:"summary"}));
    await userEvent.click(screen.getByRole("button",{name:"Open page: Project page"}));
    expect(open).toHaveBeenCalledTimes(2);expect(open).toHaveBeenLastCalledWith("p");
    expect(graphDisclosure).toHaveAttribute("open");
  });
  it("reports failed graph reads instead of asserting no connections",async()=>{
    vi.mocked(getKnowledgeGraph).mockRejectedValue(new Error("offline"));mount();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load connections");
    expect(screen.queryByText("No direct connections in the currently visible knowledge.")).toBeNull();
  });
  it("offers a keyboard graph entry even for a page without connections",async()=>{
    vi.mocked(getKnowledgeGraph).mockResolvedValue({entities:[],relations:[],memory_links:[],memories:[],pages:[{id:"p",title:"Quiet page",space:null,creation_kind:"source",entity_id:null,last_modified:""}],page_links:[]});
    const openGraph=vi.fn();
    render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><KnowledgeContext standaloneGraph kind="page" id="p" title="Quiet page" onOpenGraph={openGraph}/></QueryClientProvider>);
    const button=await screen.findByRole("button",{name:"Open in graph"});
    button.focus();await userEvent.keyboard("{Enter}");
    expect(openGraph).toHaveBeenCalledTimes(1);
  });

  it("keeps the page local graph open by default",async()=>{
    vi.mocked(getKnowledgeGraph).mockResolvedValue({entities:[],relations:[],memory_links:[],memories:[],pages:[
      {id:"p",title:"Current page",space:null,creation_kind:"source",entity_id:null,last_modified:""},
      {id:"p2",title:"Linked page",space:null,creation_kind:"source",entity_id:null,last_modified:""},
    ],page_links:[{from:{kind:"page",id:"p"},to:{kind:"page",id:"p2"},link_type:"wikilink"}]});
    render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><KnowledgeContext kind="page" id="p" title="Current page"/></QueryClientProvider>);
    await screen.findByRole("region",{name:"Linked pages"});
    expect(screen.getByText("Connections",{selector:"summary"}).closest("details")).toHaveAttribute("open");
  });

});
