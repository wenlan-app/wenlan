// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getSpace, getSpaceBrief, listEntities, listIndexedFiles, listMemoriesRich, listPages, type Page, type Space } from "../../../lib/tauri";
import SpaceDetail from "../SpaceDetail";
import { SPACE_DETAIL_TEST_COPY } from "./testTranslation";

vi.mock("../../../lib/tauri", () => ({ getSpace: vi.fn(), getSpaceBrief: vi.fn(), listPages: vi.fn(), listIndexedFiles: vi.fn(), listEntities: vi.fn(), listMemoriesRich: vi.fn(), updateSpace: vi.fn(), deleteSpace: vi.fn(), confirmSpace: vi.fn() }));
const a: Space = { id: "stable-a", name: "讀書會", description: "Reading together", suggested: false, starred: false, sort_order: 0, memory_count: 12, entity_count: 6, created_at: 1, updated_at: 1 };
function workspace() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onSelectPage = vi.fn(); const onCreatePage = vi.fn(); const onReviewAll = vi.fn();
  const result = render(<QueryClientProvider client={client}><SpaceDetail copy={SPACE_DETAIL_TEST_COPY} spaceName={a.name} onBack={() => {}} onSelectPage={onSelectPage} onCreatePage={onCreatePage} onReviewAll={onReviewAll} /></QueryClientProvider>);
  return { ...result, onSelectPage, onCreatePage, onReviewAll };
}
const page = (id: string, patch: Partial<Page> = {}): Page => ({ id, title: id, summary: null, space: a.name, last_modified: new Date(0).toISOString(), ...patch } as Page);
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getSpace).mockResolvedValue(a);
  vi.mocked(listPages).mockResolvedValue([]);
  vi.mocked(getSpaceBrief).mockResolvedValue(null);
});
describe("A Space's page", () => {
  it("UX-SPACE-1 opens with the Brief: where it stands, the last handoff and three items, the rest folded", async () => {
    vi.mocked(getSpaceBrief).mockResolvedValue({
      summary: "可信度修正都合併了。",
      handed_off_at: Math.floor(Date.now() / 1000) - 7200,
      active: ["一", "二", "三", "四", "五"].map((line, i) => ({ id: `i${i}`, line, raw: false })),
    });
    workspace();
    const brief = await screen.findByRole("region", { name: "Where this stands" });
    expect(within(brief).getByText("可信度修正都合併了。")).toBeInTheDocument();
    expect(within(brief).getByText(/^Last handoff /)).toBeInTheDocument();
    expect(within(brief).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["一", "二", "三"]);
    fireEvent.click(within(brief).getByRole("button", { name: "2 more" }));
    expect(within(brief).getAllByRole("listitem")).toHaveLength(5);
    fireEvent.click(within(brief).getByRole("button", { name: "Show less" }));
    expect(within(brief).getAllByRole("listitem")).toHaveLength(3);
    expect(getSpaceBrief).toHaveBeenCalledWith(a.name);
  });

  it("UX-SPACE-1 a Space with no Brief is its pages alone", async () => {
    vi.mocked(listPages).mockResolvedValue([page("Reading list")]);
    workspace();
    expect(await screen.findByText("Reading list")).toBeInTheDocument();
    expect(screen.queryByTestId("space-brief")).not.toBeInTheDocument();
  });

  it("UX-SPACE-3 has no Pages/Sources tabs and never loads sources, memories or topics", async () => {
    const result = workspace();
    await screen.findByRole("button", { name: "New page" });
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    expect(listIndexedFiles).not.toHaveBeenCalled();
    expect(listEntities).not.toHaveBeenCalled(); expect(listMemoriesRich).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "New page" }));
    expect(result.onCreatePage).toHaveBeenCalledExactlyOnceWith(a.name);
  });

  it("UX-SPACE-3 marks a page the AI changed in the last day, not one the person edited", async () => {
    const recent = new Date(Date.now() - 3_600_000).toISOString();
    vi.mocked(listPages).mockResolvedValue([
      page("By AI", { last_modified: recent }),
      page("By me", { last_modified: recent, human_edited_at: Math.floor(Date.now() / 1000) - 3600 }),
      page("Old", { last_modified: new Date(Date.now() - 3 * 86_400_000).toISOString() }),
    ]);
    workspace();
    const ai = (await screen.findByText("By AI")).closest("button")!;
    expect(within(ai).getByText("Updated by AI")).toBeInTheDocument();
    expect(within(screen.getByText("By me").closest("button")!).queryByText("Updated by AI")).not.toBeInTheDocument();
    expect(within(screen.getByText("Old").closest("button")!).queryByText("Updated by AI")).not.toBeInTheDocument();
  });

  it("keeps review only in the overflow menu", async () => {
    const result = workspace();
    const more = await screen.findByRole("button", { name: `Actions for ${a.name}` });
    more.focus(); fireEvent.keyDown(more, { key: "ArrowDown" });
    expect(await screen.findByRole("menuitem", { name: "Review page changes" })).toHaveFocus();
    fireEvent.click(screen.getByRole("menuitem", { name: "Review page changes" }));
    expect(result.onReviewAll).toHaveBeenCalledExactlyOnceWith();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });
});
