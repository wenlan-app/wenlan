// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import { SearchResults, matchingThings, readable } from "./SearchResults";
import type { ActivityResponse, ActivityStep, ConversationHit, EntitySearchResult, Page, SearchResult, ThingSummary } from "../../lib/tauri";

const getActivityMock = vi.hoisted(() => vi.fn());
const getConversationLinesMock = vi.hoisted(() => vi.fn());
vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...await importOriginal<typeof import("../../lib/tauri")>(),
  getActivity: getActivityMock,
  getConversationLines: getConversationLinesMock,
}));

function activity(embed?: ActivityStep): ActivityResponse {
  return {
    state: "off",
    last_activity_at: null,
    assets: [{
      kind: "memories",
      state: embed?.state ?? "idle",
      done: 3,
      total: 3,
      blocked: 0,
      steps: [
        { name: "store", state: "idle", done: 3, total: 3, failed: 0, job: null },
        ...(embed ? [embed] : []),
      ],
    }],
    everyday: { job: "everyday", lane: "none", model: null, mode: "unconfigured", available: false },
    synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
    refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
  };
}

const embed = (state: ActivityStep["state"], done: number, total: number): ActivityStep =>
  ({ name: "embed", state, done, total, failed: 0, job: null });

const result = { id: "m1", content: "delta grid checklist" } as SearchResult;

type Groups = {
  thingResults?: ThingSummary[];
  conceptResults?: Page[];
  conversationHits?: ConversationHit[];
  sourceResults?: SearchResult[];
  entityResults?: EntitySearchResult[];
};

function renderResults(data: ActivityResponse, sourceResults: SearchResult[] = [result], groups: Groups = {}, query = "delta") {
  getActivityMock.mockResolvedValue(data);
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenThing = vi.fn();
  render(
    <QueryClientProvider client={queryClient}>
      <SearchResults
        query={query}
        thingResults={groups.thingResults ?? []}
        conceptResults={groups.conceptResults ?? []}
        conversationHits={groups.conversationHits ?? []}
        sourceResults={groups.sourceResults ?? sourceResults}
        entityResults={groups.entityResults ?? []}
        loading={false}
        error={false}
        ready
        onOpenThing={onOpenThing}
        onOpenResult={() => {}}
        onOpenPage={() => {}}
        onOpenEntity={() => {}}
      />
    </QueryClientProvider>,
  );
  return { onOpenThing };
}

beforeEach(async () => {
  getActivityMock.mockReset();
  getConversationLinesMock.mockReset();
  await i18n.changeLanguage("en");
});

describe("SearchResults preparing line", () => {
  it("says how many passages are still being prepared while the step runs", async () => {
    renderResults(activity(embed("running", 700, 1000)));
    expect(await screen.findByTestId("search-preparing")).toHaveTextContent(
      "Still preparing 300 passages. Keyword matches are complete.",
    );
    expect(screen.getByTestId("search-preparing")).toHaveClass("search-modal-status");
    // Above the results, so ten results do not push it out of view.
    const line = screen.getByTestId("search-preparing");
    const firstResult = screen.getByRole("button");
    expect(line.compareDocumentPosition(firstResult) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows the line under an empty result too", async () => {
    renderResults(activity(embed("running", 0, 1)), []);
    expect(await screen.findByTestId("search-preparing")).toHaveTextContent(
      "Still preparing 1 passage. Keyword matches are complete.",
    );
    expect(screen.getByText("0 results for “delta”")).toBeInTheDocument();
  });

  it.each([
    { kind: "an idle step", data: activity(embed("idle", 0, 0)) },
    { kind: "a daemon without the step", data: activity() },
  ])("stays hidden with $kind", async ({ data }) => {
    renderResults(data);
    await vi.waitFor(() => expect(getActivityMock).toHaveBeenCalled());
    // Let the query settle before asserting the line never appears.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByRole("button")).toHaveTextContent("delta grid checklist");
    expect(screen.queryByTestId("search-preparing")).toBeNull();
  });

  it("uses the Traditional Chinese copy", async () => {
    await i18n.changeLanguage("zh-Hant");
    renderResults(activity(embed("running", 5, 17)));
    expect(await screen.findByTestId("search-preparing")).toHaveTextContent(
      "還有 12 段在準備中，關鍵字搜尋已完整。",
    );
  });
});

const thing: ThingSummary = { id: "thg_1", title: "Gion stay", title_by: "ai", status_line: "Booking the room", spaces: [], last_activity_at: 0 };
const hit: ConversationHit = {
  session_id: "ses_1",
  tool: "codex",
  thing_id: "thg_1",
  thing_title: "Gion stay",
  index: 1,
  before: { speaker: "ai", text: "Quiet, or easy to walk out at night?" },
  line: { speaker: "you", text: "Decided: stay near Gion." },
  after: { speaker: "ai", text: "Noted." },
};

describe("UX-FIND-1 one search box", () => {
  it("orders groups: working on, pages, what was said, topics", async () => {
    renderResults(activity(), [], {
      thingResults: [thing],
      conceptResults: [{ id: "p1", title: "Gion notes", summary: null, content: "Gion notes body", domain: null } as unknown as Page],
      conversationHits: [hit],
      entityResults: [{ entity: { id: "e1", name: "Gion", entity_type: "place" } } as unknown as EntitySearchResult],
    }, "gion");
    const headings = (await screen.findAllByRole("region")).map((group) => group.getAttribute("aria-label"));
    expect(headings).toEqual(["Working on", "Pages", "Said in conversations", "Topics"]);
    const said = screen.getByRole("region", { name: "Said in conversations" });
    expect(within(said).getByRole("button")).toHaveTextContent("Decided: stay near Gion.");
    expect(within(said).getByRole("button")).toHaveTextContent("You");
  });

  it("matches things by every word of their title or status line", () => {
    const other = { ...thing, id: "thg_2", title: "Tagline", status_line: null };
    expect(matchingThings([thing, other], "gion ROOM").map((t) => t.id)).toEqual(["thg_1"]);
    expect(matchingThings([thing, other], "   ")).toEqual([]);
  });

  it("reads Markdown links in a line as their text", () => {
    expect(readable("Saved in Wenlan: [Gion stay](wenlan://thing/thg_1) · done")).toBe("Saved in Wenlan: Gion stay · done");
  });
});

describe("UX-FIND-2 a sentence with what came before and after", () => {
  it("opens the sentence in context, names where it is used, and shows all on request", async () => {
    const user = userEvent.setup();
    getConversationLinesMock.mockResolvedValue({
      session_id: "ses_1",
      tool: "codex",
      lines: [
        { speaker: "you", text: "Where should we stay?" },
        { speaker: "you", text: "Decided: stay near Gion." },
        { speaker: "ai", text: "Noted." },
      ],
    });
    const { onOpenThing } = renderResults(activity(), [], { conversationHits: [hit] }, "gion");
    await user.click(await screen.findByRole("button", { name: /Decided: stay near Gion/ }));
    const quote = screen.getByTestId("conversation-quote");
    expect(quote).toHaveTextContent("Quiet, or easy to walk out at night?");
    expect(quote).toHaveTextContent("Noted.");
    expect(screen.getByRole("button", { name: "← Back to results" })).toHaveFocus();
    expect(getConversationLinesMock).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Show all" }));
    expect(await screen.findByText("Where should we stay?")).toBeInTheDocument();
    expect(getConversationLinesMock).toHaveBeenCalledWith("ses_1");
    expect(screen.queryByRole("button", { name: "Show all" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Gion stay" }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_1");

    await user.click(screen.getByRole("button", { name: "← Back to results" }));
    expect(screen.getByRole("region", { name: "Said in conversations" })).toBeInTheDocument();
  });

  it("says so when the conversation is gone from this computer", async () => {
    const user = userEvent.setup();
    getConversationLinesMock.mockRejectedValue(new Error("not found"));
    renderResults(activity(), [], { conversationHits: [hit] }, "gion");
    await user.click(await screen.findByRole("button", { name: /Decided: stay near Gion/ }));
    await user.click(screen.getByRole("button", { name: "Show all" }));
    expect(await screen.findByText("This conversation is no longer on this computer.")).toBeInTheDocument();
  });
});
