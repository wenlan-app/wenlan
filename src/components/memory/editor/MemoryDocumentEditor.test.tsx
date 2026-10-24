// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState, type Dispatch, type SetStateAction } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@testing-library/jest-dom/vitest";
import type { MemoryItem } from "../../../lib/tauri";

const editorControls = vi.hoisted(() => ({ emitDocumentChange: null as ((source: string) => void) | null }));

vi.mock("../../../lib/tauri", () => ({ getMemoryDetail: vi.fn(), updateMemory: vi.fn() }));
vi.mock("./MarkdownEditor", async () => {
  const React = await import("react");
  return {
    MarkdownEditor: (props: any) => {
      const [value, setValue] = React.useState(props.initialDocument);
      const [composing, setComposing] = React.useState(false);
      React.useEffect(() => { setValue(props.initialDocument); }, [props.sessionId, props.initialDocument]);
      const status = (compositionActive: boolean) => props.onStatusChange?.({
        sessionId: props.sessionId, engine: "native", ready: true, compositionActive,
        canUndo: false, canRedo: false,
      });
      editorControls.emitDocumentChange = source => { setValue(source); props.onDocumentChange(source); };
      return <div data-testid="markdown-editor" data-session-id={props.sessionId}>
        <textarea aria-label="Mock markdown editor" disabled={props.disabled} value={value}
          onChange={event => editorControls.emitDocumentChange?.(event.target.value)} />
        <button type="button" onClick={() => props.onSave(value)}>Mock save</button>
        <button type="button" onClick={props.onCancel}>Mock cancel</button>
        <button type="button" onClick={() => { setComposing(true); status(true); }}>Start composition</button>
        <button type="button" onClick={() => { setComposing(false); status(false); }}>End composition</button>
        <span data-testid="composition-state">{String(composing)}</span>
      </div>;
    },
  };
});

import { getMemoryDetail, updateMemory } from "../../../lib/tauri";
import { MemoryDocumentEditor } from "./MemoryDocumentEditor";

const readMemory = vi.mocked(getMemoryDetail);
const saveMemory = vi.mocked(updateMemory);

function memory(content = "Original source", sourceId = "memory-a"): MemoryItem {
  return {
    source_id: sourceId, title: "Test memory", content, summary: null, memory_type: "fact",
    domain: null, source_agent: null, confidence: 0.8, confirmed: false, pinned: false,
    supersedes: null, last_modified: 1, chunk_count: 1,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}

function setup(initial = memory()) {
  let canonical: MemoryItem | null = initial;
  readMemory.mockImplementation(async id => canonical?.source_id === id ? canonical : null);
  saveMemory.mockImplementation(async (id, content) => {
    if (canonical?.source_id === id && content !== undefined) canonical = { ...canonical, content };
  });
  let flush: (() => Promise<boolean>) | null = null;
  const onRegisterFlush = vi.fn((callback: (() => Promise<boolean>) | null) => { flush = callback; });
  const onBusyChange = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const view = render(<QueryClientProvider client={client}><MemoryDocumentEditor memory={initial} canonical={initial}
    disabled={false} onRegisterFlush={onRegisterFlush} onBusyChange={onBusyChange} /></QueryClientProvider>);
  return {
    ...view, client, onRegisterFlush, onBusyChange,
    flush: () => flush,
    rerenderEditor(nextMemory: MemoryItem, nextCanonical: MemoryItem | null | undefined) {
      view.rerender(<QueryClientProvider client={client}><MemoryDocumentEditor memory={nextMemory} canonical={nextCanonical}
        disabled={false} onRegisterFlush={onRegisterFlush} onBusyChange={onBusyChange} /></QueryClientProvider>);
    },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  editorControls.emitDocumentChange = null;
});
afterEach(() => vi.useRealTimers());

describe("MemoryDocumentEditor", () => {
  it("does not write or autofocus on initial render", () => {
    setup();
    expect(saveMemory).not.toHaveBeenCalled();
    expect(document.activeElement).not.toBe(screen.getByRole("textbox", { name: "Mock markdown editor" }));
  });

  it("debounces a direct edit and saves the exact source text", async () => {
    vi.useFakeTimers();
    setup();
    const source = "  First line\nSecond line  ";
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: source } });
    await act(async () => { await vi.advanceTimersByTimeAsync(599); });
    expect(saveMemory).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(saveMemory).toHaveBeenCalledWith("memory-a", source, undefined, undefined, undefined, "Original source");
  });

  it("retains a failed draft, reports registered flush failure, and retries explicitly", async () => {
    saveMemory.mockRejectedValueOnce(new Error("offline"));
    const { flush } = setup();
    const source = "Draft remains available";
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: source } });
    let firstResult = true;
    await act(async () => { firstResult = await flush()!(); });
    expect(firstResult).toBe(false);
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue(source);
    fireEvent.click(screen.getByRole("button", { name: "Retry saving" }));
    await waitFor(() => expect(saveMemory).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue(source);
    expect(screen.queryByRole("button", { name: "Retry saving" })).not.toBeInTheDocument();
  });

  it("preserves local typing when a dirty editor observes an external canonical change", async () => {
    const { rerenderEditor } = setup();
    const editor = screen.getByTestId("markdown-editor");
    const session = editor.getAttribute("data-session-id");
    const source = "Local unsaved draft";
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: source } });
    rerenderEditor(memory(), memory("Remote change"));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue(source);
    expect(screen.getByTestId("markdown-editor")).toHaveAttribute("data-session-id", session);
    expect(saveMemory).not.toHaveBeenCalled();
  });

  it("adopts a clean external refresh as a new editor session", async () => {
    const { rerenderEditor } = setup();
    const oldSession = screen.getByTestId("markdown-editor").getAttribute("data-session-id");
    await act(async () => { rerenderEditor(memory(), memory("Clean remote refresh")); });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue("Clean remote refresh"));
    expect(screen.getByTestId("markdown-editor").getAttribute("data-session-id")).not.toBe(oldSession);
  });

  it("disables a clean editor after external deletion and permits registered flush", async () => {
    const { rerenderEditor, flush } = setup();
    rerenderEditor(memory(), null);
    await waitFor(() => expect(screen.getByText("This memory was deleted elsewhere.")).toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toBeDisabled();
    let result = false;
    await act(async () => { result = await flush()!(); });
    expect(result).toBe(true);
    expect(saveMemory).not.toHaveBeenCalled();
  });

  it("preserves a draft on deletion until the user confirms discard", async () => {
    const { rerenderEditor, flush } = setup();
    const draft = "Keep this draft unless discard is confirmed";
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: draft } });
    rerenderEditor(memory(), null);
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue(draft);
    let blocked = true;
    await act(async () => { blocked = await flush()!(); });
    expect(blocked).toBe(false);

    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    fireEvent.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(confirm).toHaveBeenCalledWith("Discard this unsaved draft? Copy any text you want to keep first.");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toBeDisabled());
    let cleared = false;
    await act(async () => { cleared = await flush()!(); });
    expect(cleared).toBe(true);
    expect(saveMemory).not.toHaveBeenCalled();
    confirm.mockRestore();
  });

  it("does not reset the MarkdownEditor session after its own save", async () => {
    const { flush } = setup();
    const editor = screen.getByTestId("markdown-editor");
    const session = editor.getAttribute("data-session-id");
    const source = "Saved in place";
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: source } });
    fireEvent.click(screen.getByRole("button", { name: "Mock save" }));
    let result = false;
    await act(async () => { result = await flush()!(); });
    expect(result).toBe(true);
    await waitFor(() => expect(saveMemory).toHaveBeenCalledWith("memory-a", source, undefined, undefined, undefined, "Original source"));
    await waitFor(() => expect(screen.getByTestId("markdown-editor")).toHaveAttribute("data-session-id", session));
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue(source);
  });

  it("does not write during IME composition", async () => {
    vi.useFakeTimers();
    const { flush } = setup();
    fireEvent.click(screen.getByRole("button", { name: "Start composition" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: "Composing text" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    let result = true;
    await act(async () => { result = await flush()!(); });
    expect(result).toBe(false);
    expect(saveMemory).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "End composition" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(600); });
    expect(saveMemory).toHaveBeenCalledWith("memory-a", "Composing text", undefined, undefined, undefined, "Original source");
  });

  it("ignores a preflight result that arrives after unmount", async () => {
    const pendingRead = deferred<MemoryItem | null>();
    readMemory.mockReturnValueOnce(pendingRead.promise);
    const view = setup();
    // setup installs its canonical read implementation, so replace the first read after render.
    readMemory.mockReset();
    readMemory.mockReturnValueOnce(pendingRead.promise);
    fireEvent.change(screen.getByRole("textbox", { name: "Mock markdown editor" }), { target: { value: "Unmounted draft" } });
    const flush = view.flush();
    let result = true;
    const running = act(async () => { result = await flush!(); });
    await Promise.resolve();
    view.unmount();
    pendingRead.resolve(memory());
    await running;
    expect(result).toBe(false);
    expect(saveMemory).not.toHaveBeenCalled();
  });

  it("keeps a local draft mounted when canonical deletion and input batch together", async () => {
    const initial = memory();
    let setCanonical: Dispatch<SetStateAction<MemoryItem | null>> | null = null;
    readMemory.mockImplementation(async id => id === initial.source_id ? initial : null);
    saveMemory.mockImplementation(async () => {});
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const onRegisterFlush = vi.fn();

    function ParentHarness() {
      const [canonical, updateCanonical] = useState<MemoryItem | null>(initial);
      const [contentBusy, setContentBusy] = useState(false);
      setCanonical = updateCanonical;

      // Mirror MemoryDetail's deletion gate: when canonical data disappears,
      // retain the editor only if it has synchronously reported local work.
      if (canonical === null && !contentBusy) return <p role="status">Memory unavailable</p>;
      return <MemoryDocumentEditor memory={initial} canonical={canonical} disabled={false}
        onRegisterFlush={onRegisterFlush} onBusyChange={setContentBusy} />;
    }

    render(<QueryClientProvider client={client}><ParentHarness /></QueryClientProvider>);
    const draft = "Local edit in same batch as deletion";
    act(() => {
      editorControls.emitDocumentChange?.(draft);
      setCanonical?.(null);
    });

    expect(screen.queryByText("Memory unavailable")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Mock markdown editor" })).toHaveValue(draft);
    expect(saveMemory).not.toHaveBeenCalled();
  });
});
