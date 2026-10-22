// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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
import { listParts, pageCreatedMs, WritePagesCard, writePagesView } from "./WritePagesCard";

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
    const writing = writePagesView({ ...base, run, pages, list: material(9, { held: 2 }), now: START + 200_000, lastChangeAt: START });
    expect(writing).toMatchObject({ kind: "working", writing: true, stuck: false });
    expect(writing.kind === "working" && writing.written.map((p) => p.id)).toEqual(["a", "b"]);

    const between = writePagesView({ ...base, run, pages, list: material(9), now: START + 240_000, lastChangeAt: START + 200_000 });
    expect(between.kind).toBe("working");

    const done = writePagesView({ ...base, run, pages, list: material(9), now: START + 400_000, lastChangeAt: START + 200_000 });
    expect(done).toMatchObject({ kind: "done", minutes: 3, left: 9 });
  });

  it("says the tool may be waiting when nothing happens", () => {
    const run = { startedAt: START, tool: "Codex" };
    const view = writePagesView({ ...base, run, list: material(12), now: START + 130_000, lastChangeAt: START });
    expect(view).toMatchObject({ kind: "working", writing: false, stuck: true });
  });
});

describe("WritePagesCard", () => {
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
    wrap(<WritePagesCard pages={[]} onSelectPage={vi.fn()} />);
    const card = await screen.findByTestId("write-pages-card");
    expect(card).toHaveTextContent("Your material is in: 12 notes, 6 chats, and 4 files. None of it is in a page yet.");
    expect(card).toHaveTextContent("Opens Claude Code with the command typed in.");
    fireEvent.click(screen.getByRole("button", { name: "Ask AI to write pages" }));
    await waitFor(() => expect(openToolWithPrompt).toHaveBeenCalledWith("claude-code", "/wenlan steep"));
    await waitFor(() => expect(screen.getByTestId("write-pages-card")).toHaveAttribute("data-state", "working"));
    expect(screen.getByTestId("write-pages-card")).toHaveTextContent("Claude Code is writing pages");
  });

  it("names only what is left once pages exist", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(10, { kinds: { note: 2, chat: 4, file: 4 } }));
    wrap(<WritePagesCard pages={[page("a", "2026-10-22T10:01:00Z")]} onSelectPage={vi.fn()} />);
    const card = await screen.findByTestId("write-pages-card");
    expect(card).toHaveTextContent("2 notes, 4 chats, and 4 files aren't in any page yet.");
  });

  it("copies the command when the tool cannot be opened", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    vi.mocked(openToolWithPrompt).mockRejectedValue(new Error("not installed"));
    wrap(<WritePagesCard pages={[]} onSelectPage={vi.fn()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Ask AI to write pages" }));
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("$wenlan steep"));
    await waitFor(() => expect(screen.getByTestId("write-pages-card")).toHaveTextContent("The command is copied. Paste it into Codex"));
  });

  it("offers to connect a tool or use the local model when no tool is connected", async () => {
    vi.mocked(listAgents).mockResolvedValue([]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    const onConnectTool = vi.fn();
    const onUseLocalModel = vi.fn();
    wrap(<WritePagesCard pages={[]} onSelectPage={vi.fn()} onConnectTool={onConnectTool} onUseLocalModel={onUseLocalModel} />);
    fireEvent.click(await screen.findByRole("button", { name: "Connect an AI tool" }));
    fireEvent.click(screen.getByRole("button", { name: "Use local model" }));
    expect(onConnectTool).toHaveBeenCalled();
    expect(onUseLocalModel).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Ask AI to write pages" })).toBeNull();
  });

  it("stays closed once the user closes it", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(5));
    const { unmount } = wrap(<WritePagesCard pages={[]} onSelectPage={vi.fn()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Hide" }));
    expect(screen.queryByTestId("write-pages-card")).toBeNull();
    unmount();

    wrap(<WritePagesCard pages={[]} onSelectPage={vi.fn()} />);
    await waitFor(() => expect(listNewMaterial).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("write-pages-card")).toBeNull();
  });

  it("lists the pages written and opens one", async () => {
    localStorage.setItem("askAi:writePages:run", JSON.stringify({ startedAt: START, tool: "Claude Code" }));
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listNewMaterial).mockResolvedValue(material(9, { held: 3 }));
    const onSelectPage = vi.fn();
    wrap(<WritePagesCard pages={[page("a", "2026-10-22T10:01:00Z", "distilled", 8)]} onSelectPage={onSelectPage} />);
    const card = await screen.findByTestId("write-pages-card");
    expect(card).toHaveTextContent("Claude Code is writing pages: 1 ready");
    expect(card).toHaveTextContent("8 sources");
    expect(card).toHaveTextContent("Writing the next page…");
    fireEvent.click(screen.getByRole("button", { name: "Page a" }));
    expect(onSelectPage).toHaveBeenCalledWith("a");
  });
});
