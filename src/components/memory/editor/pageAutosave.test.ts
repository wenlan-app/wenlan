// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Page, UpdatePageInput, UpdatePageOutcome } from "../../../lib/tauri";
import { PageAutosave } from "./pageAutosave";

const page = (content = "Original\r\nsource", version = 3): Page => ({
  id: "page-a", title: "Page A", content, version, status: "active",
  summary: null, entity_id: null, domain: null, source_memory_ids: [],
  created_at: "2026-10-05", last_compiled: "2026-10-05", last_modified: "2026-10-05",
} as Page);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function setup() {
  let canonical = page();
  let operation = 0;
  const write = vi.fn(async (input: UpdatePageInput): Promise<UpdatePageOutcome> => {
    canonical = page(input.content, input.expectedVersion + 1);
    return { outcome: "saved" };
  });
  const read = vi.fn(async (): Promise<Page | null> => canonical);
  const changed = vi.fn();
  const confirmed = vi.fn();
  const missing = vi.fn();
  const saver = new PageAutosave({ write, read, operationId: () => `operation-${++operation}`, onChange: changed, onCanonical: confirmed, onMissing: missing });
  saver.reset(canonical);
  return { saver, write, read, changed, confirmed, missing };
}

afterEach(() => vi.useRealTimers());

describe("PageAutosave", () => {
  it("writes only user changes after the debounce and confirms exact source/version", async () => {
    vi.useFakeTimers();
    const { saver, write } = setup();
    await vi.advanceTimersByTimeAsync(1000);
    expect(write).not.toHaveBeenCalled();
    saver.setSource("Changed\r\nsource");
    await vi.advanceTimersByTimeAsync(649);
    expect(write).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(write).toHaveBeenCalledWith(expect.objectContaining({ content: "Changed\r\nsource", expectedVersion: 3 }));
    expect(saver.snapshot()).toMatchObject({ baseline: { content: "Changed\r\nsource", version: 4 }, dirty: false });
    saver.dispose();
  });

  it("saves newer input after the in-flight snapshot using its confirmed next version", async () => {
    const { saver, write, read } = setup();
    const first = deferred<UpdatePageOutcome>();
    write.mockReturnValueOnce(first.promise);
    read.mockResolvedValueOnce(page("First snapshot", 4)).mockResolvedValueOnce(page("Newer local text", 5));
    saver.setSource("First snapshot");
    const flush = saver.flush();
    saver.setSource("Newer local text");
    expect(write).toHaveBeenCalledTimes(1);
    first.resolve({ outcome: "saved" });
    expect(await flush).toBe(true);
    expect(write.mock.calls.map(([input]) => [input.content, input.expectedVersion])).toEqual([
      ["First snapshot", 3], ["Newer local text", 4],
    ]);
    expect(saver.snapshot().dirty).toBe(false);
    saver.dispose();
  });

  it("persists an undo back to the old baseline when the earlier snapshot is already in flight", async () => {
    const { saver, write, read } = setup();
    const first = deferred<UpdatePageOutcome>();
    write.mockReturnValueOnce(first.promise);
    read.mockResolvedValueOnce(page("Temporary change", 4)).mockResolvedValueOnce(page("Original\r\nsource", 5));
    saver.setSource("Temporary change");
    const flush = saver.flush();
    saver.setSource("Original\r\nsource");
    expect(saver.snapshot().dirty).toBe(true);
    first.resolve({ outcome: "saved" });
    expect(await flush).toBe(true);
    expect(write).toHaveBeenLastCalledWith(expect.objectContaining({ content: "Original\r\nsource", expectedVersion: 4 }));
    saver.dispose();
  });

  it("retries canonical confirmation without writing an already successful snapshot again", async () => {
    const { saver, write, read } = setup();
    read.mockRejectedValueOnce(new Error("offline"));
    saver.setSource("Saved remotely");
    expect(await saver.flush()).toBe(false);
    expect(saver.snapshot()).toMatchObject({ state: { phase: "retryable" }, dirty: true });
    expect(await saver.retry()).toBe(true);
    expect(write).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledTimes(2);
    saver.dispose();
  });

  it("replays an uncertain operation exactly before writing a newer draft", async () => {
    const { saver, write, read } = setup();
    write.mockRejectedValueOnce(new Error("connection lost"));
    read.mockResolvedValueOnce(page("First snapshot", 4)).mockResolvedValueOnce(page("Newer text", 5));
    saver.setSource("First snapshot");
    expect(await saver.flush()).toBe(false);
    saver.setSource("Newer text");
    expect(await saver.flush()).toBe(false);
    expect(await saver.retry()).toBe(true);
    expect(write.mock.calls[1][0]).toEqual(write.mock.calls[0][0]);
    expect(write.mock.calls[2][0]).toMatchObject({ content: "Newer text", expectedVersion: 4, operationId: "operation-2" });
    saver.dispose();
  });

  it("keeps quit protection after an ambiguous write even when local text is undone", async () => {
    const { saver, write } = setup();
    write.mockRejectedValueOnce(new Error("response lost"));
    saver.setSource("Possibly committed");
    expect(await saver.flush()).toBe(false);
    saver.setSource("Original\r\nsource");
    expect(saver.snapshot().dirty).toBe(true);
    expect(await saver.flush()).toBe(false);
    saver.dispose();
  });

  it("stops on canonical mismatch and retains both latest and the unsaved draft", async () => {
    vi.useFakeTimers();
    const { saver, write, read } = setup();
    read.mockResolvedValue(page("Changed elsewhere", 5));
    saver.setSource("Local text");
    expect(await saver.flush()).toBe(false);
    expect(saver.snapshot()).toMatchObject({ state: { phase: "conflict" }, dirty: true, latest: { content: "Changed elsewhere" } });
    saver.setSource("More local work");
    await vi.advanceTimersByTimeAsync(2000);
    expect(write).toHaveBeenCalledTimes(1);
    expect(await saver.flush()).toBe(false);
    expect(await saver.retry()).toBe(false);
    saver.dispose();
  });

  it("defers IME input and refuses navigation flush until composition ends", async () => {
    vi.useFakeTimers();
    const { saver, write } = setup();
    saver.setComposing(true);
    saver.setSource("中文輸入");
    await vi.advanceTimersByTimeAsync(1000);
    expect(await saver.flush()).toBe(false);
    expect(write).not.toHaveBeenCalled();
    saver.setComposing(false);
    await vi.advanceTimersByTimeAsync(650);
    expect(write).toHaveBeenCalledTimes(1);
    expect(saver.snapshot().dirty).toBe(false);
    saver.dispose();
  });

  it("does not apply an old page's late write to a new editor session", async () => {
    const { saver, write, confirmed } = setup();
    const pending = deferred<UpdatePageOutcome>();
    write.mockReturnValueOnce(pending.promise);
    saver.setSource("Old page edit");
    const flush = saver.flush();
    saver.reset({ ...page("New page", 1), id: "page-b" });
    pending.resolve({ outcome: "saved" });
    expect(await flush).toBe(false);
    expect(saver.snapshot()).toMatchObject({ baseline: { pageId: "page-b", content: "New page", version: 1 }, dirty: false });
    expect(confirmed).not.toHaveBeenCalled();
    saver.dispose();
  });

  it("does not repeatedly schedule an empty draft", async () => {
    vi.useFakeTimers();
    const { saver, write, changed } = setup();
    saver.setSource("");
    expect(await saver.flush()).toBe(false);
    const count = changed.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(changed).toHaveBeenCalledTimes(count);
    expect(write).not.toHaveBeenCalled();
    saver.dispose();
  });

  it("holds a deleted page in conflict even when an earlier save completes late", async () => {
    const { saver, write, confirmed } = setup();
    const pending = deferred<UpdatePageOutcome>();
    write.mockReturnValueOnce(pending.promise);
    saver.setSource("Local work still visible");
    const flushing = saver.flush();
    saver.observeCanonical(null);
    expect(saver.snapshot()).toMatchObject({ dirty: true, state: { phase: "conflict" } });
    pending.resolve({ outcome: "saved" });
    expect(await flushing).toBe(false);
    expect(await saver.flush()).toBe(false);
    expect(await saver.retry()).toBe(false);
    expect(confirmed).not.toHaveBeenCalled();
    expect(write).toHaveBeenCalledTimes(1);
    saver.dispose();
  });

  it("takes a page changed elsewhere when nothing was typed here", () => {
    const { saver, write } = setup();
    expect(saver.observeCanonical(page("Edited in Obsidian", 4))).toBe("adopt");
    expect(saver.snapshot()).toMatchObject({ state: { phase: "idle" }, dirty: false });
    // Typed and then undone back to the saved text leaves nothing to lose.
    saver.setSource("Typed");
    saver.setSource("Original\r\nsource");
    expect(saver.observeCanonical(page("Edited again", 5))).toBe("adopt");
    expect(write).not.toHaveBeenCalled();
    saver.dispose();
  });

  it("still reports a conflict when the person has unsaved text", () => {
    const { saver } = setup();
    saver.setSource("Local draft");
    expect(saver.observeCanonical(page("Edited in Obsidian", 4))).toBeUndefined();
    expect(saver.snapshot()).toMatchObject({ state: { phase: "conflict" }, latest: { content: "Edited in Obsidian" } });
    saver.dispose();
  });

  it("advances conflict observations monotonically without changing the draft operation", async () => {
    const { saver, write } = setup();
    saver.setSource("Local draft");
    saver.observeCanonical(page("Remote v4", 4));
    const pending = saver.snapshot().state;
    saver.observeCanonical(page("Remote v5", 5));
    saver.observeCanonical(page("Late v4", 4));
    saver.observeCanonical({ ...page("Other page v9", 9), id: "page-b" });
    saver.setSource("More local work");
    expect(saver.snapshot()).toMatchObject({
      baseline: { content: "Original\r\nsource", version: 3 },
      latest: { content: "Remote v5", version: 5 }, dirty: true, state: pending,
    });
    expect(await saver.flush()).toBe(false);
    expect(write).not.toHaveBeenCalled();
    saver.dispose();
  });

  it("does not let a late conflict read downgrade a newer query observation", async () => {
    const { saver, write, read } = setup();
    const reading = deferred<Page>();
    write.mockResolvedValueOnce({ outcome: "conflict", message: "Changed elsewhere" });
    read.mockReturnValueOnce(reading.promise);
    saver.setSource("Local draft");
    const flushing = saver.flush();
    await Promise.resolve();
    saver.observeCanonical(page("Remote v5", 5));
    const conflict = saver.snapshot().state;
    reading.resolve(page("Late remote v4", 4));
    expect(await flushing).toBe(false);
    expect(saver.snapshot()).toMatchObject({ latest: { content: "Remote v5", version: 5 }, state: conflict });
    saver.dispose();
  });

  it("owns a successful recovery preview through more local typing and ignores stale operations", async () => {
    const { saver, write, read } = setup();
    write.mockResolvedValueOnce({ outcome: "conflict", message: "Changed elsewhere" });
    read.mockRejectedValueOnce(new Error("offline"));
    saver.setSource("Local draft");
    expect(await saver.flush()).toBe(false);
    const state = saver.snapshot().state;
    if (state.phase !== "conflict") throw new Error("Expected conflict");
    saver.observeConflictLatest(page("Recovery v4", 4), state.pending.operationId);
    saver.observeConflictLatest(page("Wrong operation v8", 8), "stale-operation");
    saver.setSource("Local draft after recovery");
    expect(saver.snapshot()).toMatchObject({ latest: { content: "Recovery v4", version: 4 }, state });
    expect(await saver.flush()).toBe(false);
    expect(write).toHaveBeenCalledTimes(1);
    saver.dispose();
  });

  it("invalidates a started recovery read after a confirmed deletion", async () => {
    const { saver, read } = setup();
    saver.setSource("Local draft");
    saver.observeCanonical(page("Remote v4", 4));
    const state = saver.snapshot().state;
    if (state.phase !== "conflict") throw new Error("Expected conflict");
    const reading = deferred<Page>();
    read.mockReturnValueOnce(reading.promise);
    const recovery = saver.readConflictLatest(state.pending.operationId);
    saver.observeCanonical(null);
    reading.resolve(page("Stale remote v4", 4));
    expect(await recovery).toBeNull();
    expect(saver.snapshot()).toMatchObject({ latest: null, dirty: true, state: { phase: "conflict" } });
    saver.dispose();
  });

  it.each<UpdatePageOutcome>([
    { outcome: "failure", kind: "not_found", status: 404, message: "Missing" },
    { outcome: "conflict", message: "Changed elsewhere" },
    { outcome: "saved" },
  ])("confirms missing canonical source after $outcome and blocks further writes", async (outcome) => {
    const { saver, write, read, missing, confirmed } = setup();
    write.mockResolvedValueOnce(outcome);
    read.mockResolvedValueOnce(null);
    saver.setSource("Draft survives confirmed deletion");
    expect(await saver.flush()).toBe(false);
    const snapshot = saver.snapshot();
    expect(snapshot).toMatchObject({ dirty: true, latest: null, state: {
      phase: "conflict", pending: { content: "Draft survives confirmed deletion", operationId: "operation-1" },
    } });
    expect(missing).toHaveBeenCalledExactlyOnceWith("page-a");
    expect(confirmed).not.toHaveBeenCalled();
    expect(await saver.flush()).toBe(false);
    expect(await saver.retry()).toBe(false);
    expect(write).toHaveBeenCalledTimes(1);
    saver.dispose();
  });

  it("does not infer deletion when the authoritative not-found read fails", async () => {
    const { saver, write, read, missing } = setup();
    write.mockResolvedValueOnce({ outcome: "failure", kind: "not_found", status: 404, message: "Missing" });
    read.mockRejectedValueOnce(new Error("offline"));
    saver.setSource("Keep local draft");
    expect(await saver.flush()).toBe(false);
    expect(saver.snapshot()).toMatchObject({ dirty: true, state: {
      phase: "failed", pending: { content: "Keep local draft" }, failure: { kind: "not_found" },
    } });
    expect(missing).not.toHaveBeenCalled();
    expect(await saver.flush()).toBe(false);
    saver.dispose();
  });

  it("ignores a late not-found confirmation after resetting to a different page", async () => {
    const { saver, write, read, missing } = setup();
    const reading = deferred<Page | null>();
    write.mockResolvedValueOnce({ outcome: "failure", kind: "not_found", status: 404, message: "Missing" });
    read.mockReturnValueOnce(reading.promise);
    saver.setSource("Page A draft");
    const flushing = saver.flush();
    await Promise.resolve();
    expect(read).toHaveBeenCalledOnce();
    saver.reset({ ...page("Page B", 1), id: "page-b" });
    reading.resolve(null);
    expect(await flushing).toBe(false);
    expect(saver.snapshot()).toMatchObject({ baseline: { pageId: "page-b", content: "Page B" }, state: { phase: "idle" }, dirty: false });
    expect(missing).not.toHaveBeenCalled();
    saver.dispose();
  });

  it("replaces a previously loaded conflict preview with confirmed deletion", async () => {
    const { saver, read, missing } = setup();
    saver.setSource("Local draft");
    saver.observeCanonical(page("Remote v4", 4));
    const state = saver.snapshot().state;
    if (state.phase !== "conflict") throw new Error("Expected conflict");
    read.mockResolvedValueOnce(null);
    expect(await saver.readConflictLatest(state.pending.operationId)).toBeNull();
    expect(saver.snapshot()).toMatchObject({ dirty: true, latest: null, state: { phase: "conflict", pending: state.pending } });
    expect(missing).toHaveBeenCalledExactlyOnceWith("page-a");
    expect(await saver.retry()).toBe(false);
    saver.dispose();
  });

  it("confirms deletion during retry of an ambiguous saved readback without another write", async () => {
    const { saver, write, read, missing } = setup();
    read.mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(null);
    saver.setSource("Saved snapshot still protected");
    expect(await saver.flush()).toBe(false);
    expect(saver.snapshot().state.phase).toBe("retryable");
    expect(missing).not.toHaveBeenCalled();
    expect(await saver.retry()).toBe(false);
    expect(saver.snapshot()).toMatchObject({ dirty: true, state: { phase: "conflict" } });
    expect(missing).toHaveBeenCalledExactlyOnceWith("page-a");
    expect(write).toHaveBeenCalledTimes(1);
    saver.dispose();
  });

});
