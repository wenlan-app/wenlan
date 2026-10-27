// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  clipboardWrite,
  getBackgroundAiEnabled,
  listAgents,
  listNewMaterial,
  openToolWithPrompt,
  type AgentConnection,
  type NewMaterialList,
  type Page,
} from "../../../lib/tauri";
import {
  doneLine,
  listParts,
  pageCreatedMs,
  PAGES_REREAD_MS,
  pagesUpdatedSince,
  sourceCounts,
  useWritePages,
  WritePagesLine,
  writePagesView,
} from "./WritePages";

vi.mock("../../../lib/tauri", () => ({
  PROMPT_LINK_FAMILIES: ["claude-desktop", "claude-code", "codex", "cursor"],
  clipboardWrite: vi.fn(),
  getBackgroundAiEnabled: vi.fn(),
  listAgents: vi.fn(),
  listNewMaterial: vi.fn(),
  openToolWithPrompt: vi.fn(),
}));

function agent(name: string): AgentConnection {
  return {
    id: `agent-${name}`,
    name,
    display_name: null,
    agent_type: name,
    description: null,
    enabled: true,
    trust_level: "full",
    last_seen_at: 1,
    memory_count: 0,
    created_at: 1,
    updated_at: 1,
  };
}

function material(total: number, overrides: Partial<NewMaterialList> = {}): NewMaterialList {
  return { items: [], total, kinds: { note: total }, ...overrides };
}

const START = Date.parse("2026-10-22T10:00:00Z");

function page(id: string, createdAt: string, kind = "distilled", sources = 3): Page {
  return {
    id,
    title: `Page ${id}`,
    creation_kind: kind,
    created_at: createdAt,
    source_memory_ids: Array.from({ length: sources }, (_, i) => `m${i}`),
  } as Page;
}

/** The home line over its own state, as the Wiki home wires it. */
function Line({ pages, ...props }: { pages: Page[]; onConnectTool?: () => void; onUseLocalModel?: () => void }) {
  const state = useWritePages(pages);
  return <WritePagesLine state={state} pages={pages} {...props} />;
}

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{children}</QueryClientProvider>);
}

describe("pageCreatedMs", () => {
  it("reads RFC 3339, SQLite UTC text and epoch numbers", () => {
    expect(pageCreatedMs({ created_at: "2026-10-22T10:00:00Z" })).toBe(START);
    expect(pageCreatedMs({ created_at: "2026-10-22 10:00:00" })).toBe(START);
    expect(pageCreatedMs({ created_at: "2026-10-22T12:00:00+02:00" })).toBe(START);
    expect(pageCreatedMs({ created_at: String(START / 1000) })).toBe(START);
    expect(pageCreatedMs({ created_at: "x" })).toBeNull();
  });
});

describe("listParts", () => {
  it("joins Chinese with 、 and English with a conjunction", () => {
    expect(listParts(["4 份檔案", "3 條記憶"], "zh-Hant")).toBe("4 份檔案、3 條記憶");
    expect(listParts(["4 files", "3 saved memories"], "en")).toBe("4 files and 3 saved memories");
  });
});

describe("writePagesView", () => {
  const base = { onDemand: true, pages: [], run: null, dismissedTotal: null, now: START, lastChangeAt: START };

  it("offers the button On demand once there is enough material", () => {
    expect(writePagesView({ ...base, list: material(12) }).kind).toBe("ready");
    expect(writePagesView({ ...base, list: material(2) }).kind).toBe("hidden");
    expect(writePagesView({ ...base, onDemand: false, list: material(12) }).kind).toBe("hidden");
    expect(writePagesView({ ...base, onDemand: undefined, list: material(12) }).kind).toBe("hidden");
  });

  it("stays closed until much more material arrives", () => {
    expect(writePagesView({ ...base, list: material(12), dismissedTotal: 12 }).kind).toBe("hidden");
    expect(writePagesView({ ...base, list: material(21), dismissedTotal: 12 }).kind).toBe("hidden");
    expect(writePagesView({ ...base, list: material(22), dismissedTotal: 12 }).kind).toBe("ready");
  });

  it("follows a run: pages written since it started, then done once it goes quiet", () => {
    const run = { startedAt: START, tool: "Claude Code" };
    const pages = [
      page("old", "2026-10-21T09:00:00Z"),
      page("mine", "2026-10-22T10:05:00Z", "authored"),
      page("a", "2026-10-22T10:01:00Z"),
      page("b", "2026-10-22T10:03:00Z"),
    ];
    const writing = writePagesView({ ...base, run, pages, list: material(9, { held: 2 }), now: START + 200_000, lastChangeAt: START + 180_000 });
    expect(writing).toMatchObject({ kind: "working", writing: true, stuck: false });
    expect(writing.kind === "working" && writing.written.map((p: Page) => p.id)).toEqual(["a", "b"]);

    const between = writePagesView({ ...base, run, pages, list: material(9), now: START + 240_000, lastChangeAt: START + 200_000 });
    expect(between.kind).toBe("working");

    const done = writePagesView({ ...base, run, pages, list: material(9), now: START + 400_000, lastChangeAt: START + 200_000 });
    expect(done).toMatchObject({ kind: "done", minutes: 3, left: 9 });
  });

  it("counts the pages a run updated, and its minutes to the last change", () => {
    const run = { startedAt: START, tool: "Claude Code" };
    const changed = (id: string, born: string, modified: string): Page =>
      ({ ...page(id, born), last_modified: modified }) as Page;
    const pages = [
      page("a", "2026-10-22T10:01:00Z"),
      page("b", "2026-10-22T10:06:00Z"),
      changed("kitchen", "2026-10-01T09:00:00Z", "2026-10-22T10:09:00Z"),
      changed("recipes", "2026-10-02T09:00:00Z", "2026-10-22T10:04:00Z"),
      changed("untouched", "2026-10-02T09:00:00Z", "2026-10-20T10:04:00Z"),
    ];
    expect(pagesUpdatedSince(pages, START).map((p) => p.id)).toEqual(["recipes", "kitchen"]);
    const done = writePagesView({ ...base, run, pages, list: material(9), now: START + 900_000, lastChangeAt: START + 540_000 });
    expect(done).toMatchObject({ kind: "done", minutes: 9, left: 9 });
    if (done.kind !== "done") throw new Error("done");
    expect(done.written.map((p) => p.id)).toEqual(["a", "b"]);
    expect(doneLine(done, i18n.t)).toBe("Wrote 2 pages and updated 2 in 9 min.");
    expect(doneLine({ ...done, written: [] }, i18n.t)).toBe("Updated 2 pages in 9 min.");
    expect(doneLine({ ...done, updated: [] }, i18n.t)).toBe("Wrote 2 pages in 9 min.");
    expect(doneLine({ ...done, written: done.written.slice(0, 1) }, i18n.t)).toBe("Wrote 1 page and updated 2 in 9 min.");
  });

  it("counts every page the run added sources to, rewritten yet or not", () => {
    // Four claims went into pages written before the run; the list was last
    // read before three of them were rewritten.
    const before = [
      { ...page("kitchen", "2026-10-01T09:00:00Z", "distilled", 3), last_modified: "2026-10-01T09:00:00Z" },
      { ...page("garden", "2026-10-01T09:00:00Z", "distilled", 2), last_modified: "2026-10-01T09:00:00Z" },
      { ...page("money", "2026-10-01T09:00:00Z", "distilled", 4), last_modified: "2026-10-01T09:00:00Z" },
      { ...page("trip", "2026-10-01T09:00:00Z", "distilled", 5), last_modified: "2026-10-01T09:00:00Z" },
      { ...page("quiet", "2026-10-01T09:00:00Z", "distilled", 2), last_modified: "2026-10-01T09:00:00Z" },
      page("person", "2026-10-01T09:00:00Z", "entity", 2),
    ] as Page[];
    const sources = sourceCounts(before);
    expect(sources).toEqual({ kitchen: 3, garden: 2, money: 4, trip: 5, quiet: 2 });
    const grow = (p: Page, n: number, modified?: string): Page =>
      ({ ...p, source_memory_ids: Array.from({ length: n }, (_, i) => `m${i}`), ...(modified ? { last_modified: modified } : {}) }) as Page;
    const after = [
      grow(before[0], 5, "2026-10-22T10:06:00Z"),
      grow(before[1], 4),
      grow(before[2], 6),
      grow(before[3], 7),
      before[4],
      grow(before[5], 4),
      page("new", "2026-10-22T10:02:00Z"),
    ];
    expect(pagesUpdatedSince(after, START).map((p) => p.id)).toEqual(["kitchen"]);
    expect(pagesUpdatedSince(after, START, sources).map((p) => p.id).sort()).toEqual(["garden", "kitchen", "money", "trip"]);
    const run = { startedAt: START, tool: "Codex", sources };
    const done = writePagesView({ ...base, run, pages: after, list: material(9), now: START + 900_000, lastChangeAt: START + 360_000 });
    if (done.kind !== "done") throw new Error("done");
    expect(doneLine(done, i18n.t)).toBe("Wrote 1 page and updated 4 in 6 min.");
  });

  it("says the tool may be waiting when nothing happens", () => {
    const run = { startedAt: START, tool: "Codex" };
    const view = writePagesView({ ...base, run, list: material(12), now: START + 130_000, lastChangeAt: START });
    expect(view).toMatchObject({ kind: "working", writing: false, stuck: true });
  });

  it("says so too while the tool holds material but nothing comes of it", () => {
    // A tool killed right after claiming keeps its claim until it runs out.
    const run = { startedAt: START, tool: "Claude Code" };
    const held = writePagesView({ ...base, run, list: material(16, { held: 12 }), now: START + 130_000, lastChangeAt: START + 5_000 });
    expect(held).toMatchObject({ kind: "working", writing: true, stuck: true });
    const busy = writePagesView({ ...base, run, list: material(16, { held: 12 }), now: START + 60_000, lastChangeAt: START + 5_000 });
    expect(busy).toMatchObject({ kind: "working", writing: true, stuck: false });
  });
});

describe("WritePagesLine", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  beforeEach(async () => {
    localStorage.clear();
    vi.mocked(listAgents).mockReset();
    vi.mocked(listNewMaterial).mockReset();
    vi.mocked(getBackgroundAiEnabled).mockReset().mockResolvedValue(false);
    vi.mocked(openToolWithPrompt).mockReset();
    vi.mocked(clipboardWrite).mockReset().mockResolvedValue(undefined);
    await i18n.changeLanguage("en");
  });

  it("counts the material and opens the tool with the command typed in", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(22, { kinds: { note: 12, chat: 6, file: 4 } }));
    vi.mocked(openToolWithPrompt).mockResolvedValue(undefined);
    wrap(<Line pages={[]} />);
    const line = await screen.findByTestId("write-pages-line");
    expect(line).toHaveTextContent("New material: 12 notes, 6 chats, and 4 files. No page yet.");
    // One action: the copy fallback is not offered next to it.
    expect(screen.queryByRole("button", { name: "Copy command" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Have Claude Code write pages" }));
    await waitFor(() => expect(openToolWithPrompt).toHaveBeenCalledWith("claude-code", "/wenlan steep"));
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveAttribute("data-state", "working"));
    // Typed in, not yet sent: it does not claim the tool is writing.
    expect(screen.getByTestId("write-pages-line")).toHaveTextContent("Typed in Claude Code. Press Enter there to start.");
    // Only now, in case the tool did not open, the command can be copied.
    fireEvent.click(screen.getByRole("button", { name: "Didn't open? Copy the command" }));
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("/wenlan steep"));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("hands pages to the tool the user picked, worded for that tool, and remembers it", async () => {
    vi.mocked(listAgents).mockResolvedValue([{ ...agent("claude-code"), last_seen_at: 20 }, { ...agent("codex"), last_seen_at: 10 }]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    vi.mocked(openToolWithPrompt).mockResolvedValue(undefined);
    localStorage.setItem("askAi:writePages:tool", JSON.stringify("codex"));
    wrap(<Line pages={[]} />);
    fireEvent.click(await screen.findByRole("button", { name: "Have Codex write pages" }));
    await waitFor(() => expect(openToolWithPrompt).toHaveBeenCalledWith("codex", "$wenlan steep"));
  });

  it("uses the most recent tool when the picked one is no longer connected", async () => {
    vi.mocked(listAgents).mockResolvedValue([{ ...agent("claude-code"), last_seen_at: 20 }, { ...agent("codex"), last_seen_at: 10 }]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    localStorage.setItem("askAi:writePages:tool", JSON.stringify("cursor"));
    wrap(<Line pages={[]} />);
    expect(await screen.findByRole("button", { name: "Have Claude Code write pages" })).toBeInTheDocument();
  });

  it("says where to paste before copying for a tool it cannot open", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("gemini-cli")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    wrap(<Line pages={[]} />);
    const line = await screen.findByTestId("write-pages-line");
    expect(line).toHaveTextContent(/Then paste it into .+ and press Enter\./);
    fireEvent.click(screen.getByRole("button", { name: "Copy command" }));
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalled());
    expect(openToolWithPrompt).not.toHaveBeenCalled();
  });

  it("names only what is left once pages exist", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(10, { kinds: { note: 2, chat: 4, file: 4 } }));
    wrap(<Line pages={[page("a", "2026-10-22T10:01:00Z")]} />);
    const line = await screen.findByTestId("write-pages-line");
    expect(line).toHaveTextContent("2 notes, 4 chats, and 4 files aren't in any page yet.");
  });

  it("groups the digits of a big count", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(9962));
    wrap(<Line pages={[page("a", "2026-10-22T10:01:00Z")]} />);
    const line = await screen.findByTestId("write-pages-line");
    expect(line).toHaveTextContent("9,962 notes aren't in any page yet.");
  });

  it("copies the command when the tool cannot be opened", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    vi.mocked(openToolWithPrompt).mockRejectedValue(new Error("not installed"));
    wrap(<Line pages={[page("a", "2026-10-21T10:01:00Z", "distilled", 2)]} />);
    fireEvent.click(await screen.findByRole("button", { name: "Have Codex write pages" }));
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("$wenlan steep"));
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveTextContent("Paste the command into Codex and press Enter."));
    // Said once, on the button that copied it.
    expect(screen.getByTestId("write-pages-line").textContent?.match(/Copied/g)).toHaveLength(1);
    expect(JSON.parse(localStorage.getItem("askAi:writePages:run") ?? "{}")).toMatchObject({ handedOver: "copied", sources: { a: 2 } });
  });

  it.each(["en", "zh-Hans", "zh-Hant"] as const)(
    "keeps the Codex CLI action name when copying the command in %s",
    async (locale) => {
      await i18n.changeLanguage(locale);
      vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
      vi.mocked(listNewMaterial).mockResolvedValue(material(5));
      vi.mocked(openToolWithPrompt).mockRejectedValue(new Error("not installed"));
      wrap(<Line pages={[]} />);
      fireEvent.click(await screen.findByRole("button", {
        name: i18n.t("askAi.writePages.askIn", { tool: "Codex" }),
      }));
      await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("$wenlan steep"));
      await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveTextContent(
        i18n.t("askAi.writePages.pasteIt", { tool: "Codex" }),
      ));
      // Said once, on the button that copied it.
      const copied = i18n.t("askAi.copied");
      expect(screen.getByTestId("write-pages-line").textContent?.split(copied)).toHaveLength(2);
      expect(JSON.parse(localStorage.getItem("askAi:writePages:run") ?? "{}")).toMatchObject({ handedOver: "copied" });
    },
  );

  it("offers to connect a tool or use the local model when no tool is connected", async () => {
    vi.mocked(listAgents).mockResolvedValue([]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    const onConnectTool = vi.fn();
    const onUseLocalModel = vi.fn();
    wrap(<Line pages={[]} onConnectTool={onConnectTool} onUseLocalModel={onUseLocalModel} />);
    fireEvent.click(await screen.findByRole("button", { name: "Connect an AI tool" }));
    fireEvent.click(screen.getByRole("button", { name: "Use local model" }));
    expect(onConnectTool).toHaveBeenCalled();
    expect(onUseLocalModel).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /write pages$/ })).toBeNull();
  });

  it("stays closed once the user closes it", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    const { unmount } = wrap(<Line pages={[]} />);
    fireEvent.click(await screen.findByRole("button", { name: "Hide" }));
    expect(screen.queryByTestId("write-pages-line")).toBeNull();
    unmount();

    wrap(<Line pages={[]} />);
    await waitFor(() => expect(listNewMaterial).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("write-pages-line")).toBeNull();
  });

  it("says the run is done; its pages are in the home list, not here", async () => {
    // Started long enough ago that the run reads as finished.
    localStorage.setItem("askAi:writePages:run", JSON.stringify({ startedAt: Date.now() - 600_000, tool: "Claude Code" }));
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(1));
    const born = new Date(Date.now() - 300_000).toISOString();
    wrap(<Line pages={[page("a", born)]} />);
    const line = await screen.findByTestId("write-pages-line");
    expect(line).toHaveAttribute("data-state", "done");
    expect(screen.queryByRole("button", { name: "Page a" })).toBeNull();
  });

  it("says the last run stopped when its material came back with no page", async () => {
    localStorage.setItem(
      "askAi:writePages:run",
      JSON.stringify({ startedAt: Date.now() - 900_000, tool: "Claude Code", handedOver: "opened", claim: { held: 12, total: 16, at: Date.now() - 60_000 } }),
    );
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(16));
    vi.mocked(openToolWithPrompt).mockResolvedValue(undefined);
    wrap(<Line pages={[]} />);
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveAttribute("data-state", "ready"));
    expect(screen.getByTestId("write-pages-line")).toHaveTextContent("The last run stopped before writing pages.");
    expect(localStorage.getItem("askAi:writePages:run")).toBeNull();
    // A new run starts clean.
    fireEvent.click(screen.getByRole("button", { name: "Have Claude Code write pages" }));
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveAttribute("data-state", "working"));
    expect(screen.getByTestId("write-pages-line")).not.toHaveTextContent("The last run stopped");
  });

  it("says the tool may be waiting when it has held the same material for minutes with no page", async () => {
    localStorage.setItem(
      "askAi:writePages:run",
      JSON.stringify({ startedAt: Date.now() - 300_000, tool: "Claude Code", handedOver: "opened", claim: { held: 12, total: 16, at: Date.now() - 200_000 } }),
    );
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(16, { held: 12 }));
    wrap(<Line pages={[]} />);
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveTextContent("Claude Code is writing pages"));
    expect(screen.getByTestId("write-pages-line")).toHaveTextContent("Nothing happening? Check whether Claude Code is waiting for you.");
  });

  it("does not call a run stopped when its material was written up", async () => {
    localStorage.setItem(
      "askAi:writePages:run",
      JSON.stringify({ startedAt: Date.now() - 60_000, tool: "Claude Code", handedOver: "opened", claim: { held: 12, total: 16, at: Date.now() - 60_000 } }),
    );
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    // Twelve left the list: pages were written, the page list just has not caught up.
    vi.mocked(listNewMaterial).mockResolvedValue(material(4));
    wrap(<Line pages={[]} />);
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveAttribute("data-state", "working"));
    expect(screen.getByTestId("write-pages-line")).not.toHaveTextContent("The last run stopped");
    expect(localStorage.getItem("askAi:writePages:run")).not.toBeNull();
  });

  it("reads the pages again while the run goes on and when the tool lets go, though no material left", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    localStorage.setItem("askAi:writePages:run", JSON.stringify({ startedAt: Date.now() - 60_000, tool: "Claude Code", handedOver: "opened" }));
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    // Rewrites and added sources leave the waiting count where it is.
    vi.mocked(listNewMaterial).mockResolvedValue(material(9, { held: 3 }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const rereads = () => invalidate.mock.calls.filter(([filters]) => JSON.stringify(filters?.queryKey) === '["pages"]').length;
    render(
      <QueryClientProvider client={client}>
        <Line pages={[]} />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("write-pages-line")).toHaveTextContent("Claude Code is writing pages"));
    const before = rereads();
    await act(async () => {
      vi.advanceTimersByTime(PAGES_REREAD_MS);
    });
    expect(rereads()).toBe(before + 1);

    // The tool lets go of its last material: read once more at once.
    vi.mocked(listNewMaterial).mockResolvedValue(material(9, { held: 0 }));
    await act(async () => {
      vi.advanceTimersByTime(4_000);
    });
    await waitFor(() => expect(rereads()).toBeGreaterThanOrEqual(before + 2));
  });

  it("follows the run in one line", async () => {
    localStorage.setItem("askAi:writePages:run", JSON.stringify({ startedAt: START, tool: "Claude Code", handedOver: "opened" }));
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(9, { held: 3 }));
    wrap(<Line pages={[page("a", "2026-10-22T10:01:00Z", "distilled", 8)]} />);
    const line = await screen.findByTestId("write-pages-line");
    expect(line).toHaveTextContent("Claude Code is writing pages: 1 ready");
    expect(screen.queryByRole("button", { name: "Page a" })).toBeNull();
    // A page landed, so the tool did open: no copy fallback.
    expect(screen.queryByRole("button", { name: /Copy the command/ })).toBeNull();
  });
});
