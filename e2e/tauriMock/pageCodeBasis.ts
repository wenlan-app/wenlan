// SPDX-License-Identifier: AGPL-3.0-only
import type {
  CodeBasisCited,
  CodeBasisConfirmResult,
  CodeBasisLocator,
  CodeBasisPendingEntry,
  CodeBasisRef,
  CodeBasisState,
  CodeBasisUnavailableReason,
  Page as KnowledgePage,
} from "../../src/lib/tauri";

/** One "this note relies on these lines of code" row, as the daemon stores it. */
export type MockCodeBasisRow = {
  readonly pageId: string;
  readonly repoRoot: string;
  readonly path: string;
  readonly lineStart: number;
  readonly lineEnd: number;
  readonly state: CodeBasisState;
  readonly changedAt?: number;
  readonly reason?: CodeBasisUnavailableReason;
  /** The git blob of the whole file as the note cited it (40 hex characters); null for an older row. */
  readonly citedBlob: string | null;
  /** The next confirm answers "it changed again" once, then works. */
  readonly changesAgain?: boolean;
};

/**
 * Fixture state for code bases. `files` maps an absolute file path to the text
 * it reads now; a path that is absent cannot be read. `blobs` maps a git blob
 * id to the file text it holds (a blob that is absent cannot be read, as when
 * git has lost the object). Confirming a row stores a blob of the file as it
 * reads now as its cited blob and returns it to `ok`.
 */
export type TauriMockCodeBasisScenario = {
  readonly rows: readonly MockCodeBasisRow[];
  readonly files?: Readonly<Record<string, string>>;
  readonly blobs?: Readonly<Record<string, string>>;
};

const sameRow = (row: MockCodeBasisRow, pageId: string, locator: CodeBasisLocator) =>
  row.pageId === pageId
  && row.repoRoot === locator.repo_root
  && row.path === locator.path
  && row.lineStart === locator.line_start
  && row.lineEnd === locator.line_end;

export class PageCodeBasisStore {
  private rows: MockCodeBasisRow[];
  private readonly blobs: Record<string, string>;
  private blobCount = 0;

  constructor(private readonly scenario: TauriMockCodeBasisScenario = { rows: [] }) {
    this.rows = scenario.rows.map((row) => ({ ...row }));
    this.blobs = { ...scenario.blobs };
  }

  refsFor(pageId: string): CodeBasisRef[] {
    return this.rows.filter((row) => row.pageId === pageId).map(refOf);
  }

  pending(space: string | null, pages: readonly KnowledgePage[]): CodeBasisPendingEntry[] {
    const entries: CodeBasisPendingEntry[] = [];
    for (const page of pages) {
      if (page.status !== "active") continue;
      if (space && (page.space ?? page.domain) !== space) continue;
      const pending = this.rows
        .filter((row) => row.pageId === page.id && (row.state === "pending" || row.state === "unavailable"))
        .map(refOf);
      if (pending.length > 0) entries.push({ page_id: page.id, title: page.title, pending });
    }
    return entries.sort((a, b) => a.title.localeCompare(b.title));
  }

  confirm(pageId: string, locator: CodeBasisLocator): CodeBasisConfirmResult {
    const row = this.rows.find((entry) => sameRow(entry, pageId, locator));
    if (!row) return { outcome: "conflict", message: "this note does not rely on those lines" };
    if (row.changesAgain) {
      this.replace(row, { ...row, changesAgain: false });
      return { outcome: "conflict", message: "the file changed again" };
    }
    const now = this.read(row.repoRoot, row.path);
    if (now === null) return { outcome: "conflict", message: "the file cannot be read now" };
    this.blobCount += 1;
    const blob = this.blobCount.toString(16).padStart(40, "e");
    this.blobs[blob] = now;
    const confirmed: MockCodeBasisRow = {
      ...row,
      state: "ok",
      changedAt: undefined,
      reason: undefined,
      citedBlob: blob,
    };
    this.replace(row, confirmed);
    return { outcome: "confirmed", code_basis: refOf(confirmed) };
  }

  remove(pageId: string, locator: CodeBasisLocator): { removed: boolean } {
    const before = this.rows.length;
    this.rows = this.rows.filter((row) => !sameRow(row, pageId, locator));
    return { removed: this.rows.length !== before };
  }

  cited(pageId: string, locator: CodeBasisLocator): CodeBasisCited {
    const row = this.rows.find((entry) => sameRow(entry, pageId, locator));
    if (!row) throw new Error("this note does not rely on those lines (404)");
    return { cited_blob: row.citedBlob, confirmed_at: 1_783_700_000, state: row.state };
  }

  /** The `read_git_blob_lines` command: the requested lines of a blob, or the error a lost object gives. */
  readBlobLines(blob: string, lineStart: number, lineEnd: number): string[] {
    const text = this.blobs[blob];
    if (text === undefined) throw new Error("git could not find that version of the file");
    return sliceLines(text, lineStart, lineEnd);
  }

  /** The `read_code_lines` command: the requested lines of a file, or the error a missing file gives. */
  readLines(path: string, lineStart: number, lineEnd: number): string[] {
    const text = this.scenario.files?.[path];
    if (text === undefined) throw new Error(`No such file or directory (os error 2): ${path}`);
    return sliceLines(text, lineStart, lineEnd);
  }

  private read(repoRoot: string, path: string): string | null {
    return this.scenario.files?.[`${repoRoot.replace(/[\\/]+$/, "")}/${path}`] ?? null;
  }

  private replace(row: MockCodeBasisRow, next: MockCodeBasisRow) {
    this.rows = this.rows.map((entry) => (entry === row ? next : entry));
  }
}

function sliceLines(text: string, lineStart: number, lineEnd: number): string[] {
  const lines = text.split(/\r?\n/);
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines.slice(lineStart - 1, lineEnd);
}

function refOf(row: MockCodeBasisRow): CodeBasisRef {
  return {
    repo_root: row.repoRoot,
    path: row.path,
    line_start: row.lineStart,
    line_end: row.lineEnd,
    state: row.state,
    ...(row.changedAt !== undefined ? { changed_at: row.changedAt } : {}),
    ...(row.reason !== undefined ? { reason: row.reason } : {}),
  };
}
