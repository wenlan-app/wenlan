// SPDX-License-Identifier: AGPL-3.0-only
import type { Page, UpdatePageInput, UpdatePageOutcome } from "../../../lib/tauri";
import {
  beginPageSave,
  settlePageSave,
  type PageEditBaseline,
  type PageSaveCoordinatorState,
  type PendingPageSave,
} from "./pageSaveCoordinator";

export interface PageAutosaveSnapshot {
  baseline: PageEditBaseline | null;
  state: PageSaveCoordinatorState;
  dirty: boolean;
  validation: "empty" | null;
  latest: Page | null;
}

interface PageAutosaveOptions {
  write(input: UpdatePageInput): Promise<UpdatePageOutcome>;
  read(id: string): Promise<Page | null>;
  operationId(): string;
  onChange(snapshot: PageAutosaveSnapshot): void;
  onCanonical(page: Page): void;
  onMissing?(pageId: string): void;
  delayMs?: number;
}

/** Serial snapshot writes. The editor document and its undo history stay outside. */
export class PageAutosave {
  private baseline: PageEditBaseline | null = null;
  private source = "";
  private state: PageSaveCoordinatorState = { phase: "idle" };
  private latest: Page | null = null;
  private validation: "empty" | null = null;
  private composing = false;
  private awaitingCanonical: PendingPageSave | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private running: Promise<boolean> | null = null;
  private generation = 0;

  constructor(private options: PageAutosaveOptions) {}

  setObserver(onChange: PageAutosaveOptions["onChange"]): void {
    this.options.onChange = onChange;
  }

  snapshot(): PageAutosaveSnapshot {
    return {
      baseline: this.baseline,
      state: this.state,
      dirty: this.baseline !== null && (
        this.source !== this.baseline.content ||
        // A retryable operation may already have committed remotely, even if
        // the user has since undone local text back to the old baseline.
        this.state.phase !== "idle" || this.awaitingCanonical !== null
      ),
      validation: this.validation,
      latest: this.latest,
    };
  }

  reset(page: Page | null): void {
    this.cancelTimer();
    this.generation += 1;
    this.running = null;
    this.baseline = page ? { pageId: page.id, content: page.content, version: page.version } : null;
    this.source = page?.content ?? "";
    this.state = { phase: "idle" };
    this.latest = null;
    this.awaitingCanonical = null;
    this.validation = null;
    this.composing = false;
    this.emit();
  }

  dispose(): void {
    this.cancelTimer();
    this.generation += 1;
    this.running = null;
  }

  setSource(source: string): void {
    if (!this.baseline || source === this.source) return;
    this.source = source;
    this.validation = null;
    this.emit();
    this.schedule();
  }

  setComposing(composing: boolean): void {
    this.composing = composing;
    if (composing) this.cancelTimer();
    else this.schedule();
  }

  /**
   * Returns "adopt" when the page changed elsewhere and nothing was typed
   * here since: the caller then opens the newer page instead of a conflict.
   */
  observeCanonical(page: Page | null): "adopt" | void {
    const baseline = this.baseline;
    if (!baseline) return;
    if (page === null) {
      // A confirmed deletion is a conflict even when an older write is still
      // completing. Its late response cannot make this local draft clean.
      this.cancelTimer();
      this.generation += 1;
      this.running = null;
      this.awaitingCanonical = null;
      this.latest = null;
      this.state = {
        phase: "conflict",
        pending: this.state.phase === "idle"
          ? { operationId: this.options.operationId(), content: this.source, expectedVersion: baseline.version }
          : this.state.pending,
        message: "The canonical page no longer exists.",
      };
      this.emit();
      return;
    }
    if (page.id !== baseline.pageId) return;
    if (this.state.phase === "conflict") {
      this.observeConflictLatest(page, this.state.pending.operationId);
      return;
    }
    if (this.state.phase !== "idle") return;
    if (page.version < baseline.version) return;
    if (page.content === baseline.content) {
      if (page.version !== baseline.version) {
        this.baseline = { ...baseline, version: page.version };
        this.emit();
      }
      return;
    }
    if (
      this.source === baseline.content && !this.composing && this.timer === null &&
      this.running === null && this.awaitingCanonical === null
    ) return "adopt";
    this.cancelTimer();
    this.latest = page;
    this.state = {
      phase: "conflict",
      pending: { operationId: this.options.operationId(), content: this.source, expectedVersion: baseline.version },
      message: "The canonical page changed while its editor was open.",
    };
    this.emit();
  }

  /** Conflict recovery reads and query observations share one monotonic owner. */
  observeConflictLatest(page: Page | null, operationId: string): Page | null {
    if (
      this.state.phase !== "conflict" ||
      this.state.pending.operationId !== operationId ||
      !this.baseline
    ) return null;
    if (
      page && page.id === this.baseline.pageId &&
      page.version >= this.baseline.version &&
      (!this.latest || page.version > this.latest.version)
    ) {
      this.latest = page;
      this.emit();
    }
    return this.latest;
  }

  async readConflictLatest(operationId: string): Promise<Page | null> {
    if (
      !this.baseline || this.state.phase !== "conflict" ||
      this.state.pending.operationId !== operationId
    ) return null;
    const generation = this.generation;
    const pageId = this.baseline.pageId;
    const page = await this.options.read(pageId);
    // A confirmed deletion/reset invalidates an already-started recovery read.
    if (generation !== this.generation) return null;
    if (page === null) {
      this.confirmMissing(generation, pageId, operationId);
      return null;
    }
    return this.observeConflictLatest(page, operationId);
  }

  flush(): Promise<boolean> {
    this.cancelTimer();
    if (this.composing) return Promise.resolve(false);
    if (this.running) return this.running;
    if (this.state.phase !== "idle") return Promise.resolve(false);
    return this.start();
  }

  retry(): Promise<boolean> {
    this.cancelTimer();
    if (this.composing || this.state.phase === "conflict") return Promise.resolve(false);
    if (this.running) return this.running;
    let replay: PendingPageSave | undefined;
    if (this.state.phase !== "idle" && this.state.phase !== "pending") {
      // An ambiguous transport/server failure must resolve the exact operation
      // before a newer draft gets a fresh operation id and canonical version.
      if (this.state.phase === "retryable") replay = this.state.pending;
    }
    this.state = { phase: "idle" };
    return this.start(replay);
  }

  private start(replay?: PendingPageSave): Promise<boolean> {
    const generation = this.generation;
    const run = this.drain(generation, replay).finally(() => {
      if (this.running === run) {
        this.running = null;
        if (generation === this.generation) this.schedule();
      }
    });
    this.running = run;
    return run;
  }

  private async drain(generation: number, replay?: PendingPageSave): Promise<boolean> {
    if (this.awaitingCanonical) {
      const pending = this.awaitingCanonical;
      this.state = { phase: "pending", pending };
      this.emit();
      if (!await this.reconcile(generation, pending)) return false;
      replay = undefined;
    }
    while (generation === this.generation && this.baseline) {
      if (this.composing || this.state.phase !== "idle") return false;
      if (!replay && this.source === this.baseline.content) return true;
      const attempt = replay
        ? {
            kind: "request" as const,
            input: {
              id: this.baseline.pageId,
              content: replay.content,
              expectedVersion: replay.expectedVersion,
              operationId: replay.operationId,
              callerId: "wenlan-app" as const,
            },
            state: { phase: "pending" as const, pending: replay },
          }
        : beginPageSave(this.state, this.baseline, this.source, this.options.operationId);
      replay = undefined;
      if (attempt.kind !== "request") {
        if (attempt.kind === "invalid") this.validation = "empty";
        this.emit();
        return attempt.kind === "unchanged";
      }
      const pending = attempt.state.pending;
      this.state = attempt.state;
      this.emit();
      let outcome: UpdatePageOutcome;
      try {
        outcome = await this.options.write(attempt.input);
      } catch {
        if (generation !== this.generation) return false;
        this.state = settlePageSave(this.state, { outcome: "transport" });
        this.emit();
        return false;
      }
      if (generation !== this.generation) return false;
      if (outcome.outcome !== "saved") {
        this.state = settlePageSave(this.state, outcome);
        this.emit();
        if (outcome.outcome === "conflict" || (outcome.outcome === "failure" && outcome.kind === "not_found")) {
          try {
            const latest = await this.options.read(attempt.input.id);
            if (generation !== this.generation) return false;
            if (latest === null) {
              this.confirmMissing(generation, attempt.input.id, pending.operationId);
              return false;
            }
            this.observeConflictLatest(latest, pending.operationId);
          } catch { /* The local draft remains available if latest cannot load. */ }
          if (generation === this.generation) this.emit();
        }
        return false;
      }
      this.awaitingCanonical = pending;
      if (!await this.reconcile(generation, pending)) return false;
    }
    return generation === this.generation;
  }

  private async reconcile(generation: number, pending: PendingPageSave): Promise<boolean> {
    const baseline = this.baseline;
    if (!baseline) return false;
    let canonical: Page | null;
    try {
      canonical = await this.options.read(baseline.pageId);
      if (canonical && canonical.id !== baseline.pageId) throw new Error("Mismatched canonical page");
    } catch {
      if (generation !== this.generation) return false;
      this.state = { phase: "retryable", pending, failure: { outcome: "transport" } };
      this.emit();
      return false;
    }
    if (generation !== this.generation) return false;
    if (canonical === null) {
      this.confirmMissing(generation, baseline.pageId, pending.operationId);
      return false;
    }
    this.awaitingCanonical = null;
    if (canonical.content !== pending.content || canonical.version <= pending.expectedVersion) {
      this.latest = canonical;
      this.state = { phase: "conflict", pending, message: "The canonical source does not match the saved snapshot." };
      this.emit();
      return false;
    }
    this.baseline = { pageId: canonical.id, content: canonical.content, version: canonical.version };
    this.state = { phase: "idle" };
    this.validation = null;
    this.emit();
    this.options.onCanonical(canonical);
    return true;
  }

  private confirmMissing(generation: number, pageId: string, operationId: string): void {
    if (
      generation !== this.generation || this.baseline?.pageId !== pageId ||
      this.state.phase === "idle" || this.state.pending.operationId !== operationId
    ) return;
    this.observeCanonical(null);
    this.options.onMissing?.(pageId);
  }

  private schedule(): void {
    this.cancelTimer();
    if (this.composing || this.running || this.validation || this.state.phase !== "idle" || !this.snapshot().dirty) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, this.options.delayMs ?? 650);
  }

  private cancelTimer(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  private emit(): void { this.options.onChange(this.snapshot()); }
}
