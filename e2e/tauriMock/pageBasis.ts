// SPDX-License-Identifier: AGPL-3.0-only
import type {
  BasisConfirmResult,
  BasisDiffEdit,
  BasisDiffResult,
  BasisPendingEntry,
  BasisRef,
  BasisState,
  Page as KnowledgePage,
} from "../../src/lib/tauri";

/** One "this note relies on that note" row, as the daemon stores it. */
export type MockBasisRow = {
  readonly pageId: string;
  readonly basisPageId: string;
  /** The title the basis had when last seen; what a deleted basis is still called. */
  readonly basisTitle: string;
  /** The version of the basis the note was last confirmed against. */
  readonly fromVersion: number;
  /** What the basis said at `fromVersion`; the left side of the diff. */
  readonly fromContent: string;
};

/**
 * Fixture state for note bases. States are never stored: like the daemon's
 * list, they are worked out from the basis note's current status, space and
 * version, so editing, archiving or deleting a basis in a spec is enough.
 */
export type TauriMockBasisScenario = {
  readonly rows: readonly MockBasisRow[];
  /** The edits a basis went through, newest last, keyed by the basis note. */
  readonly edits?: Readonly<Record<string, readonly BasisDiffEdit[]>>;
  /** What "Update from sources" does for a note: stage a preview card or find nothing new. */
  readonly redistill?: Readonly<Record<string, { readonly gated: string } | "unchanged">>;
};

export class PageBasisStore {
  private rows: MockBasisRow[];

  constructor(private readonly scenario: TauriMockBasisScenario = { rows: [] }) {
    this.rows = scenario.rows.map((row) => ({ ...row }));
  }

  redistillOutcome(pageId: string): { readonly gated: string } | "unchanged" | null {
    return this.scenario.redistill?.[pageId] ?? null;
  }

  refsFor(pageId: string, pages: readonly KnowledgePage[]): BasisRef[] {
    return this.rows
      .filter((row) => row.pageId === pageId)
      .map((row) => this.refOf(row, pages));
  }

  mark(pageId: string, basisPageId: string, pages: readonly KnowledgePage[]): BasisRef {
    const basis = pages.find((page) => page.id === basisPageId);
    if (!basis) throw new Error("the basis note was not found (404)");
    if (pageId === basisPageId) throw new Error("a note cannot be its own basis (409)");
    const row: MockBasisRow = {
      pageId,
      basisPageId,
      basisTitle: basis.title,
      fromVersion: basis.version,
      fromContent: basis.content,
    };
    this.rows = [
      ...this.rows.filter((existing) => !(existing.pageId === pageId && existing.basisPageId === basisPageId)),
      row,
    ];
    return this.refOf(row, pages);
  }

  unmark(pageId: string, basisPageId: string): { removed: boolean } {
    const before = this.rows.length;
    this.rows = this.rows.filter((row) => !(row.pageId === pageId && row.basisPageId === basisPageId));
    return { removed: this.rows.length !== before };
  }

  confirm(
    pageId: string,
    basisPageId: string,
    version: number,
    pages: readonly KnowledgePage[],
  ): BasisConfirmResult {
    const row = this.rows.find((entry) => entry.pageId === pageId && entry.basisPageId === basisPageId);
    const basis = pages.find((page) => page.id === basisPageId);
    if (!row || !basis) return { outcome: "conflict", message: "this note does not rely on that note" };
    if (basis.version !== version) {
      return { outcome: "conflict", message: `the basis is at version ${basis.version}, not ${version}` };
    }
    const confirmed: MockBasisRow = { ...row, fromVersion: version, fromContent: basis.content };
    this.rows = this.rows.map((entry) => (entry === row ? confirmed : entry));
    return { outcome: "confirmed", basis: this.refOf(confirmed, pages) };
  }

  /** Confirming through a save: the same check, reported the way the save route reports it. */
  confirmFromSave(
    pageId: string,
    input: { readonly basis_page_id: string; readonly version: number },
    pages: readonly KnowledgePage[],
  ): "confirmed" | "moved_on" {
    const result = this.confirm(pageId, input.basis_page_id, input.version, pages);
    return result.outcome === "confirmed" ? "confirmed" : "moved_on";
  }

  diff(pageId: string, basisPageId: string, pages: readonly KnowledgePage[]): BasisDiffResult {
    const row = this.rows.find((entry) => entry.pageId === pageId && entry.basisPageId === basisPageId);
    const basis = pages.find((page) => page.id === basisPageId);
    if (!row) return { outcome: "unavailable", reason: "not_a_basis", message: "this note does not rely on that note" };
    if (!basis) return { outcome: "unavailable", reason: "deleted", message: "the basis note was deleted" };
    if (basis.status !== "active") return { outcome: "unavailable", reason: "archived", message: "the basis note is archived" };
    const citer = pages.find((page) => page.id === pageId);
    if (citer && (citer.space ?? null) !== (basis.space ?? null)) {
      return { outcome: "unavailable", reason: "other_space", message: "the two notes are in different Spaces" };
    }
    return {
      outcome: "available",
      diff: {
        from_version: row.fromVersion,
        to_version: basis.version,
        from_content: row.fromContent,
        to_content: basis.content,
        edits: this.editsSince(row),
      },
    };
  }

  pending(space: string | null, pages: readonly KnowledgePage[]): BasisPendingEntry[] {
    const groups = new Map<string, BasisPendingEntry>();
    for (const row of this.rows) {
      const citer = pages.find((page) => page.id === row.pageId);
      if (!citer || citer.status !== "active") continue;
      if (space && (citer.space ?? citer.domain) !== space) continue;
      const ref = this.refOf(row, pages);
      if (ref.state === "ok") continue;
      const key = `${row.basisPageId}\u0000${ref.state}`;
      const group = groups.get(key) ?? {
        basis_page_id: row.basisPageId,
        title: ref.title,
        changed_version: ref.to_version ?? null,
        state: ref.state,
        citers: [],
      };
      group.citers.push({ page_id: citer.id, title: citer.title, minor_only: ref.minor_only === true });
      groups.set(key, group);
    }
    return [...groups.values()]
      .map((group) => ({
        ...group,
        citers: [...group.citers].sort((a, b) => a.title.localeCompare(b.title)),
      }))
      .sort((a, b) => a.title.localeCompare(b.title));
  }

  private editsSince(row: MockBasisRow): BasisDiffEdit[] {
    return (this.scenario.edits?.[row.basisPageId] ?? []).filter((edit) => edit.version > row.fromVersion);
  }

  private refOf(row: MockBasisRow, pages: readonly KnowledgePage[]): BasisRef {
    const basis = pages.find((page) => page.id === row.basisPageId);
    const citer = pages.find((page) => page.id === row.pageId);
    if (!basis) {
      return {
        page_id: row.basisPageId,
        title: row.basisTitle,
        state: "deleted",
        from_version: row.fromVersion,
      };
    }
    let state: BasisState = "ok";
    if (basis.status !== "active") state = "archived";
    else if (citer && (citer.space ?? null) !== (basis.space ?? null)) state = "unavailable";
    else if (basis.version > row.fromVersion) state = "pending";
    const edits = this.editsSince(row);
    return {
      page_id: row.basisPageId,
      title: state === "unavailable" ? "" : basis.title,
      state,
      from_version: row.fromVersion,
      to_version: basis.version,
      ...(state === "pending" ? { minor_only: edits.length > 0 && edits.every((edit) => edit.minor) } : {}),
    };
  }
}
