// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import type { EntityDetail as EntityDetailRecord, SearchResult } from "../../lib/tauri";
import EntityDetail from "./EntityDetail";

vi.mock("../../lib/tauri", () => ({
  getEntityDetail: vi.fn(),
  updateObservation: vi.fn().mockResolvedValue(undefined),
  deleteObservation: vi.fn().mockResolvedValue(undefined),
  addObservation: vi.fn().mockResolvedValue("obs-new"),
  confirmObservation: vi.fn().mockResolvedValue(undefined),
  confirmEntity: vi.fn().mockResolvedValue(undefined),
  deleteEntity: vi.fn().mockResolvedValue(undefined),
  search: vi.fn(),
  FACET_COLORS: { fact: "facet-fact" },
}));

vi.mock("./FocusGraph", () => ({
  default: (props: { onEntityClick?: (id: string) => void }) => (
    <div data-testid="focus-graph">
      <button type="button" onClick={() => props.onEntityClick?.("entity-babbage")}>
        Focus node
      </button>
    </div>
  ),
}));

vi.mock("./AtlasView", () => ({
  default: (props: {
    focusEntityId?: string;
    onNodeClick?: (target: { kind: "entity" | "memory"; id: string }) => void;
  }) => (
    <div data-focus-entity-id={props.focusEntityId ?? ""} data-testid="atlas-view">
      <button
        type="button"
        onClick={() => props.onNodeClick?.({ kind: "entity", id: "entity-hopper" })}
      >
        Atlas node
      </button>
      <button
        type="button"
        onClick={() => props.onNodeClick?.({ kind: "memory", id: "memory-ada" })}
      >
        Atlas memory node
      </button>
    </div>
  ),
}));

import {
  addObservation,
  confirmObservation,
  confirmEntity,
  deleteEntity,
  deleteObservation,
  getEntityDetail,
  search,
  updateObservation,
} from "../../lib/tauri";

const detail: EntityDetailRecord = {
  entity: {
    id: "entity-ada",
    name: "Ada Lovelace",
    entity_type: "person",
    domain: "computing",
    space: "History of Computing",
    source_agent: "research-agent",
    confidence: 0.87,
    confirmed: false,
    created_at: 1_700_000_000,
    updated_at: 1_700_086_400,
    memory_count: 0,
    status: "detected",
    established_by: null,
  },
  observations: [
    {
      id: "obs-1",
      entity_id: "entity-ada",
      content: "Wrote the first published algorithm",
      source_agent: "research-agent",
      confidence: 0.8,
      confirmed: false,
      created_at: 1_700_000_100,
    },
  ],
  relations: [
    {
      id: "relation-1",
      relation_type: "collaborated with",
      direction: "outgoing",
      entity_id: "entity-babbage",
      entity_name: "Charles Babbage",
      entity_type: "person",
      source_agent: "research-agent",
      created_at: 1_700_000_200,
    },
    {
      id: "relation-2",
      relation_type: "inspired",
      direction: "incoming",
      entity_id: "entity-hopper",
      entity_name: "Grace Hopper",
      entity_type: "person",
      source_agent: null,
      created_at: 1_700_000_300,
    },
  ],
};

const linkedMemory: SearchResult = {
  id: "chunk-1",
  content: "Ada's notes described a general-purpose machine.",
  source: "notes.md",
  source_id: "memory-ada",
  title: "Ada's notes",
  url: null,
  chunk_index: 0,
  last_modified: 1_700_086_500,
  score: 0.94,
  memory_type: "fact",
  entity_id: "entity-ada",
  is_archived: false,
};

const defaultProps = {
  entityId: "entity-ada",
  onBack: vi.fn(),
  onEntityClick: vi.fn(),
  onMemoryClick: vi.fn(),
  onRegisterFlush: undefined as ((flush: (() => Promise<boolean>) | null) => void) | undefined,
};

function renderEntity(props = defaultProps) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
      mutations: { retry: false },
    },
  });
  return {
    user: userEvent.setup(),
    ...render(
      <QueryClientProvider client={queryClient}>
        <EntityDetail {...props} />
      </QueryClientProvider>,
    ),
  };
}

describe("EntityDetail characterization", () => {
  afterEach(() => vi.restoreAllMocks());
  beforeEach(async () => {
    // jsdom has no layout. Represent the visible controls in these focus tests;
    // real hidden/visible geometry is covered by the browser regressions.
    vi.spyOn(HTMLElement.prototype, "getClientRects").mockImplementation(
      () => [new DOMRect(0, 0, 100, 30)] as unknown as DOMRectList,
    );
    vi.clearAllMocks();
    await i18n.changeLanguage("en");
    vi.mocked(getEntityDetail).mockResolvedValue(detail);
    vi.mocked(search).mockResolvedValue([linkedMemory]);
  });

  it("loads the entity record with identity, metadata, observations, relations, and linked memories", async () => {
    const { user } = renderEntity();

    expect(await screen.findByRole("heading", { name: "Ada Lovelace" })).toBeInTheDocument();
    expect(getEntityDetail).toHaveBeenCalledWith("entity-ada");
    await user.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByRole("combobox", { name: "Type" })).toHaveValue("person");
    expect(screen.getAllByText("History of Computing").length).toBeGreaterThan(0);
    expect(screen.getByText("Research Agent")).toBeInTheDocument();
    expect(screen.getByText("0.87")).toBeInTheDocument();
    expect(screen.getByText("Wrote the first published algorithm")).toBeInTheDocument();
    expect(screen.getAllByText("Charles Babbage").length).toBeGreaterThan(0);
    expect(
      await screen.findByText("Ada's notes described a general-purpose machine."),
    ).toBeInTheDocument();
  });

  it("exposes back navigation and recovers from a load error", async () => {
    vi.mocked(getEntityDetail)
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue(detail);
    const { user } = renderEntity();

    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(defaultProps.onBack).toHaveBeenCalledOnce();
    expect(await screen.findByText("Couldn't load this topic.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("heading", { name: "Ada Lovelace" })).toBeInTheDocument();
    expect(getEntityDetail).toHaveBeenCalledTimes(2);
  });

  it("opens and focuses an entry directly without a global edit mode", async () => {
    const { user } = renderEntity();
    const content = await screen.findByRole("button", { name: "Edit information — Wrote the first published algorithm" });
    const entry = content.closest("li")!;
    expect(entry).toBeInTheDocument();
    await user.click(content);
    const input = screen.getByRole("textbox", { name: "Edit information" });
    expect(input).toHaveValue("Wrote the first published algorithm");
    expect(input).toHaveFocus();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(updateObservation).not.toHaveBeenCalled();
  });

  it("keeps topic retention in Info and does not mutate metadata when opening it", async () => {
    const { user } = renderEntity();

    await user.click(await screen.findByRole("button", { name: "Topic actions" }));
    expect(screen.queryByRole("menuitem", { name: "Info" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitemcheckbox")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByRole("dialog", { name: "Info" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Keep topic" })).toBeEnabled();
    expect(confirmEntity).not.toHaveBeenCalled();
    expect(confirmObservation).not.toHaveBeenCalled();
    expect(deleteObservation).not.toHaveBeenCalled();
  });

  it("autosaves an existing observation after the pause and keeps its editor open", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });
    await user.click(await screen.findByRole("button", { name: "Edit information — Wrote the first published algorithm" }));
    const editInput = screen.getByRole("textbox", { name: "Edit information" });
    await user.clear(editInput);
    await user.type(editInput, "Edited observation");
    await waitFor(() => expect(updateObservation).toHaveBeenCalledExactlyOnceWith("obs-1", "Edited observation"), { timeout: 1500 });
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("Edited observation");
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveFocus();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("serializes a latest draft typed while an earlier write is in flight", async () => {
    let resolveFirst: (() => void) | undefined;
    vi.mocked(updateObservation).mockImplementationOnce(() => new Promise<void>((resolve) => { resolveFirst = resolve; }));
    let registeredFlush: (() => Promise<boolean>) | null = null;
    const onRegisterFlush = vi.fn((flush: (() => Promise<boolean>) | null) => { registeredFlush = flush; });
    const { user } = renderEntity({ ...defaultProps, onRegisterFlush });
    await user.click(await screen.findByRole("button", { name: "Edit information — Wrote the first published algorithm" }));
    const input = screen.getByRole("textbox", { name: "Edit information" });
    await user.clear(input);
    await user.type(input, "First value");
    const pendingFlush = registeredFlush!();
    await waitFor(() => expect(updateObservation).toHaveBeenCalledExactlyOnceWith("obs-1", "First value"));
    await user.type(input, " then latest");
    resolveFirst?.();
    expect(await pendingFlush).toBe(true);
    expect(updateObservation).toHaveBeenCalledTimes(2);
    expect(updateObservation).toHaveBeenLastCalledWith("obs-1", "First value then latest");
    expect(input).toHaveValue("First value then latest");
  });

  it("waits for IME composition to end before autosaving", async () => {
    const { user } = renderEntity();
    await user.click(await screen.findByRole("button", { name: "Edit information — Wrote the first published algorithm" }));
    const input = screen.getByRole("textbox", { name: "Edit information" });
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "組み立て中" } });
    await new Promise((resolve) => setTimeout(resolve, 650));
    expect(updateObservation).not.toHaveBeenCalled();
    fireEvent.compositionEnd(input);
    await waitFor(() => expect(updateObservation).toHaveBeenCalledExactlyOnceWith("obs-1", "組み立て中"), { timeout: 1500 });
  });

  it("flushes the current draft before Back and registers a flush only while mounted", async () => {
    const props = { ...defaultProps, onBack: vi.fn() };
    let registeredFlush: (() => Promise<boolean>) | null = null;
    const onRegisterFlush = vi.fn((flush: (() => Promise<boolean>) | null) => { registeredFlush = flush; });
    const { user, unmount } = renderEntity({ ...props, onRegisterFlush });
    await screen.findByRole("heading", { name: "Ada Lovelace" });

    await user.click(screen.getByRole("button", { name: "Edit information — Wrote the first published algorithm" }));
    const input = screen.getByRole("textbox", { name: "Edit information" });
    await user.clear(input);
    await user.type(input, "Saved while leaving the entry");
    expect(registeredFlush).toBeTypeOf("function");
    expect(await registeredFlush!()).toBe(true);
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Edit information — Saved while leaving the entry" }));
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("Saved while leaving the entry");

    await user.click(screen.getByRole("button", { name: "Back" }));

    expect(updateObservation).toHaveBeenCalledExactlyOnceWith(
      "obs-1",
      "Saved while leaving the entry",
    );
    expect(props.onBack).toHaveBeenCalledOnce();
    unmount();
    expect(onRegisterFlush).toHaveBeenLastCalledWith(null);
  });

  it("inserts a newline on Enter and flushes on Escape without reverting", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });

    await user.click(screen.getByRole("button", { name: "Edit information — Wrote the first published algorithm" }));
    const input = screen.getByRole("textbox", { name: "Edit information" });
    await user.clear(input);
    await user.type(input, "Line one");
    await user.keyboard("{End}{Enter}Line two");
    expect(input).toHaveValue("Line one\nLine two");
    expect(updateObservation).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(updateObservation).toHaveBeenCalledExactlyOnceWith("obs-1", "Line one\nLine two"));
    expect(screen.queryByRole("textbox", { name: "Edit information" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit information — Line one\nLine two" })).toBeInTheDocument();
  });

  it("saves the active entry before switching and keeps its draft after a failed switch save", async () => {
    vi.mocked(getEntityDetail).mockResolvedValue({
      ...detail,
      observations: [
        detail.observations[0],
        { ...detail.observations[0], id: "obs-2", content: "Second observation" },
      ],
    });
    vi.mocked(updateObservation)
      .mockRejectedValueOnce(new Error("write failed"))
      .mockResolvedValueOnce(undefined);
    const { user } = renderEntity();
    const first = await screen.findByRole("button", { name: "Edit information — Wrote the first published algorithm" });
    await user.click(first);
    const input = screen.getByRole("textbox", { name: "Edit information" });
    await user.clear(input);
    await user.type(input, "Keep this draft");

    await user.click(screen.getByRole("button", { name: "Edit information — Second observation" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't save. Try again.");
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("Keep this draft");
    expect(updateObservation).toHaveBeenCalledExactlyOnceWith("obs-1", "Keep this draft");

    await user.click(screen.getByRole("button", { name: "Edit information — Second observation" }));
    expect(updateObservation).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("Keep this draft");

    await user.click(screen.getByRole("button", { name: "Retry save" }));
    await waitFor(() => expect(updateObservation).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("Keep this draft");

    await user.click(screen.getByRole("button", { name: "Edit information — Second observation" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("Second observation"));
  });

  it("preserves duplicate entries, hides legacy blank rows, and offers an empty add entry", async () => {
    vi.mocked(getEntityDetail).mockResolvedValue({
      ...detail,
      observations: [
        detail.observations[0],
        {
          ...detail.observations[0],
          id: "obs-2",
          content: detail.observations[0].content,
        },
        { ...detail.observations[0], id: "obs-3", content: "" },
      ],
    });
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });
    expect(screen.getAllByRole("button", { name: "Edit information — Wrote the first published algorithm" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Edit information — Add information about Ada Lovelace…" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add information" }));
    expect(screen.getByRole("textbox", { name: "Add information" })).toHaveValue("");
    expect(confirmObservation).not.toHaveBeenCalled();
  });

  it("deletes the selected observation with one hover-revealed trash activation", async () => {
    vi.mocked(getEntityDetail).mockResolvedValue({
      ...detail,
      observations: [
        detail.observations[0],
        { ...detail.observations[0], id: "obs-2", content: "Keep this separate entry" },
      ],
    });
    const { user } = renderEntity();
    const selected = await screen.findByRole("button", {
      name: "Edit information — Wrote the first published algorithm",
    });
    const selectedRow = selected.closest("li")!;
    await user.hover(selectedRow);
    const remove = within(selectedRow).getByRole("button", {
      name: "Delete information — Wrote the first published algorithm",
    });
    await user.click(remove);

    await waitFor(() => expect(deleteObservation).toHaveBeenCalledExactlyOnceWith("obs-1"));
    expect(screen.queryByRole("button", {
      name: "Edit information — Wrote the first published algorithm",
    })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit information — Keep this separate entry" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("deletes an entry when its cleared editor is left", async () => {
    const { user } = renderEntity();
    await user.click(await screen.findByRole("button", {
      name: "Edit information — Wrote the first published algorithm",
    }));
    const input = screen.getByRole("textbox", { name: "Edit information" });
    await user.clear(input);
    await user.keyboard("{Escape}");

    await waitFor(() => expect(deleteObservation).toHaveBeenCalledExactlyOnceWith("obs-1"));
    expect(updateObservation).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox", { name: "Edit information" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", {
      name: "Edit information — Wrote the first published algorithm",
    })).not.toBeInTheDocument();
    expect(defaultProps.onBack).not.toHaveBeenCalled();
  });

  it("adds one new observation then updates that returned canonical ID", async () => {
    let registeredFlush: (() => Promise<boolean>) | null = null;
    const onRegisterFlush = vi.fn((flush: (() => Promise<boolean>) | null) => { registeredFlush = flush; });
    const { user } = renderEntity({ ...defaultProps, onRegisterFlush });
    await screen.findByRole("heading", { name: "Ada Lovelace" });
    await user.click(screen.getByRole("button", { name: "Add information" }));
    const input = screen.getByRole("textbox", { name: "Add information" });
    await user.type(input, "新しい情報");
    expect(await registeredFlush!()).toBe(true);
    expect(addObservation).toHaveBeenCalledExactlyOnceWith("entity-ada", "新しい情報", "human", 1);
    await user.type(screen.getByRole("textbox", { name: "Add information" }), " updated");
    expect(await registeredFlush!()).toBe(true);
    expect(addObservation).toHaveBeenCalledTimes(1);
    expect(updateObservation).toHaveBeenCalledExactlyOnceWith("obs-new", "新しい情報 updated");
    await user.keyboard("{Escape}");
    const persisted = screen.getByRole("button", { name: "Edit information — 新しい情報 updated" });
    await user.click(persisted);
    expect(screen.getByRole("textbox", { name: "Edit information" })).toHaveValue("新しい情報 updated");
    expect(addObservation).toHaveBeenCalledTimes(1);
  });

  it("closes an empty new draft without creating a row and blocks short additions", async () => {
    let registeredFlush: (() => Promise<boolean>) | null = null;
    const onRegisterFlush = vi.fn((flush: (() => Promise<boolean>) | null) => { registeredFlush = flush; });
    const { user } = renderEntity({ ...defaultProps, onRegisterFlush });
    await user.click(await screen.findByRole("button", { name: "Add information" }));
    let input = screen.getByRole("textbox", { name: "Add information" });
    await user.type(input, "   ");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("textbox", { name: "Add information" })).not.toBeInTheDocument();
    expect(addObservation).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Add information" }));
    input = screen.getByRole("textbox", { name: "Add information" });
    await user.type(input, "四字內容");
    expect(await registeredFlush!()).toBe(false);
    expect(await screen.findByRole("alert")).toHaveTextContent("Write at least 5 characters");
    expect(addObservation).not.toHaveBeenCalled();
  });

  it("routes graph, ledger, and linked-memory controls through their existing callbacks", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });

    await user.click(screen.getByRole("button", { name: "Info" }));
    const charlesControls = screen.getAllByRole("button", { name: /Charles Babbage/ });
    expect(charlesControls).toHaveLength(2);
    await user.click(charlesControls[0]);
    await user.click(charlesControls[1]);
    await user.click(
      await screen.findByRole("button", {
        name: /Ada's notes described a general-purpose machine/,
      }),
    );

    expect(defaultProps.onEntityClick).toHaveBeenNthCalledWith(1, "entity-babbage");
    expect(defaultProps.onEntityClick).toHaveBeenNthCalledWith(2, "entity-babbage");
    expect(defaultProps.onMemoryClick).toHaveBeenCalledWith("memory-ada");
  });

  it("opens the Focus graph overlay and closes it on Escape", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });

    await user.click(screen.getByRole("button", { name: "Info" }));
    await user.click(screen.getByRole("button", { name: "Full screen" }));

    expect(screen.getByRole("dialog", { name: "Full screen" })).toBeInTheDocument();
    expect(screen.getByTestId("focus-graph")).toBeInTheDocument();
    const graphDialog = screen.getByRole("dialog", { name: "Full screen" });
    const graphClose = within(graphDialog).getByRole("button", { name: "Close" });
    expect(graphClose).toHaveFocus();
    await user.keyboard("{Shift>}{Tab}{/Shift}");
    expect(graphDialog).toContainElement(document.activeElement as HTMLElement);
    expect(graphClose).not.toHaveFocus();
    await user.keyboard("{Tab}");
    expect(graphClose).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Full screen" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Info" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Full screen" })).toHaveFocus();
    expect(defaultProps.onBack).not.toHaveBeenCalled();
  });

  it("switches the overlay to Atlas with the current entity focused", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });
    await user.click(screen.getByRole("button", { name: "Info" }));
    await user.click(screen.getByRole("button", { name: "Full screen" }));
    const dialog = screen.getByRole("dialog", { name: "Full screen" });

    await user.click(within(dialog).getByRole("button", { name: "Atlas" }));
    expect(await screen.findByTestId("atlas-view")).toHaveAttribute(
      "data-focus-entity-id",
      "entity-ada",
    );

    await user.click(within(dialog).getByRole("button", { name: "Atlas node" }));
    expect(defaultProps.onEntityClick).toHaveBeenCalledWith("entity-hopper");
    expect(screen.queryByRole("dialog", { name: "Full screen" })).not.toBeInTheDocument();
  });

  it("routes a memory node in the Atlas overlay to the memory, closing the overlay", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });
    await user.click(screen.getByRole("button", { name: "Info" }));
    await user.click(screen.getByRole("button", { name: "Full screen" }));
    const dialog = screen.getByRole("dialog", { name: "Full screen" });
    await user.click(within(dialog).getByRole("button", { name: "Atlas" }));
    await screen.findByTestId("atlas-view");

    await user.click(within(dialog).getByRole("button", { name: "Atlas memory node" }));

    expect(defaultProps.onMemoryClick).toHaveBeenCalledWith("memory-ada");
    expect(defaultProps.onEntityClick).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: "Full screen" })).not.toBeInTheDocument();
  });

  it("uses a two-step entity delete and returns after success", async () => {
    const { user } = renderEntity();
    await screen.findByRole("heading", { name: "Ada Lovelace" });

    await user.click(screen.getByRole("button", { name: "Topic actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Delete topic" }));
    expect(deleteEntity).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: /^Delete$/ }));

    await waitFor(() => {
      expect(deleteEntity).toHaveBeenCalledWith("entity-ada");
      expect(defaultProps.onBack).toHaveBeenCalledOnce();
    });
  });
});
