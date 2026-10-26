// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  getBackgroundAiEnabled,
  getOnDeviceModel,
  getResolvedRouting,
  setBackgroundAiEnabled,
  type Page,
  type ResolvedRouting,
} from "../../../lib/tauri";
import { fillUnsetPinsWithRetry } from "../../../lib/routingPins";
import {
  ImportedLead,
  ImportOrganize,
  OrganizeActionsContext,
  turnIntoPagesKind,
  TurnIntoPagesView,
  type OrganizeActions,
} from "./TurnIntoPages";
import type { WritePagesState, WritePagesView } from "./WritePages";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getBackgroundAiEnabled: vi.fn(),
  setBackgroundAiEnabled: vi.fn(async () => undefined),
  getResolvedRouting: vi.fn(),
  getOnDeviceModel: vi.fn(),
  listAgents: vi.fn(async () => []),
  listNewMaterial: vi.fn(async () => ({ items: [], total: 0 })),
  listPagesExplicitBrowse: vi.fn(async () => []),
}));

vi.mock("../../../lib/routingPins", () => ({
  fillUnsetPinsWithRetry: vi.fn(async () => null),
}));

const START = Date.parse("2026-10-22T10:00:00Z");

function page(id: string, createdAt = "2026-10-22T10:02:00Z"): Page {
  return { id, title: `Page ${id}`, creation_kind: "distilled", created_at: createdAt, source_memory_ids: ["m1"] } as Page;
}

function state(view: WritePagesView, overrides: Partial<WritePagesState> = {}): WritePagesState {
  return {
    view,
    stepView: view,
    cardView: view,
    waitingTotal: 12,
    onDemand: true,
    loaded: true,
    run: null,
    tool: { family: "codex", name: "Codex" },
    tools: [{ family: "codex", name: "Codex" }],
    chooseTool: vi.fn(),
    toolName: "Codex",
    opensTool: true,
    command: "$wenlan steep",
    copied: false,
    waiting: "12 notes",
    start: vi.fn(async () => undefined),
    copyCommand: vi.fn(async () => true),
    close: vi.fn(),
    ...overrides,
  };
}

function actions(): OrganizeActions {
  return { onConnectTool: vi.fn(), onOpenIntelligence: vi.fn(), onOpenActivity: vi.fn(), onSelectPage: vi.fn() };
}

function wrap(children: ReactNode, value: OrganizeActions | null = actions()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <OrganizeActionsContext.Provider value={value}>{children}</OrganizeActionsContext.Provider>
    </QueryClientProvider>
  );
}

const ready: WritePagesView = { kind: "ready", list: { items: [], total: 12 } };

describe("TurnIntoPagesView", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.clearAllMocks();
  });

  it("hands the notes to a tool it can open, by that tool's name", () => {
    const s = state(ready);
    render(wrap(<TurnIntoPagesView state={s} hasPages={false} />));
    const card = screen.getByTestId("turn-into-pages");
    expect(card).toHaveAttribute("data-state", "ready");
    expect(screen.getByRole("heading", { name: "Turn them into pages" })).toBeInTheDocument();
    expect(card).toHaveTextContent(
      "Codex writes your first 8 pages from your newest topics, with people, projects and links filled in. About 5 minutes, on your Codex plan.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Write pages with Codex" }));
    expect(s.start).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Switch to Automatic" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Later" })).toBeInTheDocument();
    expect(card.textContent).not.toMatch(/Claude|steep/i);
  });

  it("says the last run stopped before writing pages, over the same button", () => {
    const s = state(ready, { stopped: true });
    render(wrap(<TurnIntoPagesView state={s} hasPages={false} />));
    expect(screen.getByTestId("turn-into-pages-stopped")).toHaveTextContent("The last run stopped before writing pages.");
    fireEvent.click(screen.getByRole("button", { name: "Write pages with Codex" }));
    expect(s.start).toHaveBeenCalled();
  });

  it("says the tool may be waiting while it holds notes and nothing lands", () => {
    render(wrap(
      <TurnIntoPagesView
        state={state(
          { kind: "working", written: [], writing: true, stuck: true },
          { run: { startedAt: Date.now(), tool: "Codex", handedOver: "opened" } },
        )}
        hasPages={false}
      />,
    ));
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("Nothing happening? Check whether Codex is waiting for you.");
  });

  it("copies the command for a tool with no prompt link, and says where to paste it", () => {
    render(wrap(
      <TurnIntoPagesView
        state={state(ready, { tool: { family: "kimi-code", name: "Kimi Code" }, toolName: "Kimi Code", opensTool: false })}
        hasPages={false}
      />,
    ));
    expect(screen.getByRole("button", { name: "Copy command for Kimi Code" })).toBeInTheDocument();
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("Then paste it into Kimi Code and press Enter.");
  });

  it("lets the user pick another connected tool, and offers no pick with one tool", () => {
    const tools = [{ family: "codex", name: "Codex" }, { family: "kimi-code", name: "Kimi Code" }];
    const s = state(ready, { tools });
    const { unmount } = render(wrap(<TurnIntoPagesView state={s} hasPages={false} />));
    const toggle = screen.getByRole("button", { name: "Use another tool" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(toggle);
    expect(screen.getByRole("menuitemradio", { name: "Codex" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Kimi Code" }));
    expect(s.chooseTool).toHaveBeenCalledWith("kimi-code");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    unmount();
    render(wrap(<TurnIntoPagesView state={state(ready)} hasPages={false} />));
    expect(screen.queryByRole("button", { name: "Use another tool" })).not.toBeInTheDocument();
  });

  it("says the next pages, not the first, once the library has pages", () => {
    render(wrap(<TurnIntoPagesView state={state(ready)} hasPages />));
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("Codex writes 8 more pages from your newest topics");
  });

  it("asks for a tool when none is connected", () => {
    const value = actions();
    render(wrap(<TurnIntoPagesView state={state(ready, { tool: null, toolName: "Your AI tool" })} hasPages={false} />, value));
    expect(screen.getByTestId("turn-into-pages")).toHaveAttribute("data-state", "no-tool");
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("Connect your AI tool to write pages, or switch to Automatic.");
    fireEvent.click(screen.getByRole("button", { name: "Connect an AI tool" }));
    expect(value.onConnectTool).toHaveBeenCalled();
  });

  it("follows the run as pages land, out of 8, and opens one", () => {
    const value = actions();
    const written = [page("a"), page("b"), page("c")];
    render(wrap(
      <TurnIntoPagesView
        state={state(
          { kind: "working", written, writing: true, stuck: false },
          { run: { startedAt: Date.now(), tool: "Codex", handedOver: "opened" } },
        )}
        hasPages
      />,
      value,
    ));
    const card = screen.getByTestId("turn-into-pages");
    expect(card).toHaveTextContent("Codex is writing pages: 3 of 8 ready");
    expect(card).toHaveTextContent("Writing the next page…");
    expect(card).toHaveTextContent("You can do something else. Pages show up here as they're written.");
    fireEvent.click(screen.getByRole("button", { name: "Page b" }));
    expect(value.onSelectPage).toHaveBeenCalledWith("b");
  });

  it("says what is ready and what still waits, with one press for more", () => {
    const written = Array.from({ length: 8 }, (_, i) => page(`p${i}`));
    const s = state(
      { kind: "done", written, updated: [], minutes: 3, left: 12 },
      { run: { startedAt: Date.now(), tool: "Codex", handedOver: "opened" } },
    );
    render(wrap(<TurnIntoPagesView state={s} hasPages />));
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("8 pages ready. 12 notes still waiting.");
    fireEvent.click(screen.getByRole("button", { name: "Write 8 more" }));
    expect(s.start).toHaveBeenCalled();
  });

  it("offers the next batch after a run started outside the app, with no tool known", () => {
    const written = Array.from({ length: 8 }, (_, i) => page(`p${i}`));
    const s = state(
      { kind: "done", written, updated: [], minutes: 3, left: 9943 },
      { tool: null, opensTool: false, waiting: "9,943 notes", run: { startedAt: Date.now(), tool: null } },
    );
    render(wrap(<TurnIntoPagesView state={s} hasPages />));
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("8 pages ready. 9,943 notes still waiting.");
    fireEvent.click(screen.getByRole("button", { name: "Write 8 more" }));
    expect(s.start).toHaveBeenCalled();
  });

  it("drops the waiting half when nothing waits", () => {
    const written = Array.from({ length: 8 }, (_, i) => page(`p${i}`));
    render(wrap(
      <TurnIntoPagesView
        state={state({ kind: "done", written, updated: [], minutes: 3, left: 0 }, { waitingTotal: 0, run: { startedAt: Date.now(), tool: "Codex" } })}
        hasPages
      />,
    ));
    const card = screen.getByTestId("turn-into-pages");
    expect(card).toHaveTextContent("8 pages ready.");
    expect(card).not.toHaveTextContent("still waiting");
    expect(screen.queryByRole("button", { name: "Write 8 more" })).not.toBeInTheDocument();
  });

  it("explains Automatic before changing anything, then switches and opens the model picker", async () => {
    const value = actions();
    render(wrap(<TurnIntoPagesView state={state(ready)} hasPages={false} />, value));
    fireEvent.click(screen.getByRole("button", { name: "Switch to Automatic" }));
    const card = screen.getByTestId("turn-into-pages");
    expect(card).toHaveAttribute("data-state", "explain");
    expect(card).toHaveTextContent(
      "Automatic: Wenlan keeps organizing in the background with a model you pick. A local model is free and runs on this computer, but it is slower and writes less well than Codex.",
    );
    expect(setBackgroundAiEnabled).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Keep On demand" }));
    expect(screen.getByTestId("turn-into-pages")).toHaveAttribute("data-state", "ready");

    fireEvent.click(screen.getByRole("button", { name: "Switch to Automatic" }));
    fireEvent.click(screen.getByRole("button", { name: "Switch to Automatic" }));
    await waitFor(() => expect(value.onOpenIntelligence).toHaveBeenCalled());
    expect(setBackgroundAiEnabled).toHaveBeenCalledWith(true);
    expect(fillUnsetPinsWithRetry).toHaveBeenCalled();
  });

  it("tells the screen when it shows a button, so the screen's own primary steps back", () => {
    const onActionChange = vi.fn();
    const { unmount } = render(wrap(<TurnIntoPagesView state={state(ready)} hasPages={false} onActionChange={onActionChange} />));
    expect(onActionChange).toHaveBeenLastCalledWith(true);
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(onActionChange).toHaveBeenLastCalledWith(false);
    unmount();

    const working = vi.fn();
    render(wrap(
      <TurnIntoPagesView
        state={state({ kind: "working", written: [page("a")], writing: true, stuck: false }, { run: { startedAt: Date.now(), tool: "Codex", handedOver: "opened" } })}
        hasPages
        onActionChange={working}
      />,
    ));
    expect(working).toHaveBeenLastCalledWith(false);
  });

  it("goes away on Later", () => {
    render(wrap(<TurnIntoPagesView state={state(ready)} hasPages={false} />));
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(screen.queryByTestId("turn-into-pages")).not.toBeInTheDocument();
  });
});

describe("turnIntoPagesKind", () => {
  const base = { loaded: true, onDemand: true as const, run: null, tool: { family: "codex", name: "Codex" }, waitingTotal: 4 };

  it("hides in Automatic, before the tools are known, and with nothing waiting", () => {
    expect(turnIntoPagesKind({ ...base, onDemand: false, cardView: ready }, START)).toBe("hidden");
    expect(turnIntoPagesKind({ ...base, loaded: false, cardView: ready }, START)).toBe("hidden");
    expect(turnIntoPagesKind({ ...base, waitingTotal: 0, cardView: { kind: "hidden" } }, START)).toBe("hidden");
  });

  it("offers the new material instead of a run that finished long before the import", () => {
    const earlier = { startedAt: START - 2 * 60 * 60_000, tool: "Codex" };
    const done: WritePagesView = { kind: "done", written: [page("a")], updated: [], minutes: 2, left: 4 };
    expect(turnIntoPagesKind({ ...base, run: earlier, cardView: done }, START)).toBe("ready");
    expect(turnIntoPagesKind({ ...base, run: { ...earlier, startedAt: START - 5 * 60_000 }, cardView: done }, START)).toBe("done");
  });

  it("follows an earlier run only while its tool is still writing", () => {
    const earlier = { startedAt: START - 2 * 60 * 60_000, tool: "Codex" };
    const stalled: WritePagesView = { kind: "working", written: [], writing: false, stuck: true };
    const writing: WritePagesView = { kind: "working", written: [], writing: true, stuck: false };
    expect(turnIntoPagesKind({ ...base, run: earlier, cardView: stalled }, START)).toBe("ready");
    expect(turnIntoPagesKind({ ...base, run: earlier, cardView: writing }, START)).toBe("working");
    expect(turnIntoPagesKind({ ...base, tool: null, cardView: ready }, START)).toBe("noTool");
  });
});

describe("ImportOrganize", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.clearAllMocks();
  });

  it("names the import with the right noun", () => {
    render(wrap(<><ImportedLead count={12} kind="note" /><ImportedLead count={1} kind="chat" /><ImportedLead count={3} kind="file" /></>));
    const leads = screen.getAllByTestId("import-lead").map((node) => node.textContent);
    expect(leads).toEqual([
      "12 notes imported. You can search them now.",
      "1 chat imported. You can search them now.",
      "3 files imported. You can search them now.",
    ]);
  });

  it("writes big counts with the locale's grouping", async () => {
    const { unmount } = render(wrap(<ImportedLead count={10000} kind="note" />));
    expect(screen.getByTestId("import-lead")).toHaveTextContent("10,000 notes imported. You can search them now.");
    unmount();
    const written = Array.from({ length: 8 }, (_, i) => page(`p${i}`));
    render(wrap(<TurnIntoPagesView state={state({ kind: "done", written, updated: [], minutes: 3, left: 9962 }, { waiting: i18n.t("askAi.writePages.kinds.note", { count: 9962 }) })} hasPages />));
    expect(screen.getByTestId("turn-into-pages")).toHaveTextContent("8 pages ready. 9,962 notes still waiting.");
    for (const lng of ["zh-Hans", "zh-Hant"]) {
      await i18n.changeLanguage(lng);
      expect(i18n.t("askAi.writePages.kinds.note", { count: 10000 })).toContain("10,000");
      expect(i18n.t("askAi.writePages.importCard.done", { count: 1200 })).toContain("1,200");
    }
  });

  it("says only the second half under a heading that already gives the count", () => {
    render(wrap(<ImportedLead count={12} kind="note" short />));
    expect(screen.getByTestId("import-lead")).toHaveTextContent(/^You can search them now\.$/);
  });

  it("in Automatic, names the model writing pages and links to Activity, with no button", async () => {
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(true);
    vi.mocked(getResolvedRouting).mockResolvedValue({
      everyday: { source: "on_device", model: "qwen3-4b", mode: "pinned", pin: "on_device" },
      synthesis: { source: "on_device", model: "qwen3-4b", mode: "pinned", pin: "on_device" },
      pool: { anthropic: { configured: false, everyday_model: null, synthesis_model: null }, external: null, on_device: { selected: "qwen3-4b", loaded: true } },
    } as ResolvedRouting);
    vi.mocked(getOnDeviceModel).mockResolvedValue({
      loaded: "qwen3-4b",
      selected: "qwen3-4b",
      models: [{ id: "qwen3-4b", display_name: "Qwen3 4B", param_count: "4B", ram_required_gb: 4, file_size_gb: 2.5, cached: true }],
    });
    const value = actions();
    render(wrap(<ImportOrganize count={12} kind="note" />, value));
    await waitFor(() => {
      expect(screen.getByTestId("turn-pages-automatic")).toHaveTextContent(
        "Wenlan is turning them into pages with Qwen3 4B. First pages in a few minutes.",
      );
    });
    expect(screen.queryByTestId("turn-into-pages")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "See progress on Activity" }));
    expect(value.onOpenActivity).toHaveBeenCalled();
  });

  it("in Automatic with an unknown model, says your chosen model", async () => {
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(true);
    vi.mocked(getResolvedRouting).mockResolvedValue(null);
    render(wrap(<ImportOrganize count={12} kind="note" />));
    await waitFor(() => {
      expect(screen.getByTestId("turn-pages-automatic")).toHaveTextContent(
        "Wenlan is turning them into pages with your chosen model.",
      );
    });
  });
});
