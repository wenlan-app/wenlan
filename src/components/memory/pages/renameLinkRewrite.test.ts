// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyTextEdits,
  clearRenamedLinks,
  recordRenamedLinks,
  renamedLinkEdits,
  renameRewriteExplaining,
} from "./renameLinkRewrite";

const rewrite = (content: string, from = "Old", to = "New") =>
  applyTextEdits(content, renamedLinkEdits(content, from, to));

afterEach(() => clearRenamedLinks());

describe("rename link rewrite", () => {
  it("rewrites every link form the daemon rewrites, keeping heading, label and spacing", () => {
    expect(rewrite("[[Old]] [[ old ]] [[OLD|label]] [[Old#Part]] ![[Old]] [[Other]]"))
      .toBe("[[New]] [[ New ]] [[New|label]] [[New#Part]] ![[New]] [[Other]]");
  });

  it("leaves code, escaped brackets and Space-qualified links as written", () => {
    const content = "`[[Old]]` \\[[Old]] [[Work/Old]]\n```\n[[Old]]\n```\n[[Old]]";
    expect(rewrite(content)).toBe("`[[Old]]` \\[[Old]] [[Work/Old]]\n```\n[[Old]]\n```\n[[New]]");
  });

  it("reads code spans the same with CRLF and LF line endings", () => {
    // A backtick with its closer past a blank line is a literal backtick, so
    // both links are links, whichever line endings the text uses.
    const lf = "`a\n\n[[Old]] b` [[Old]]";
    const crlf = lf.replaceAll("\n", "\r\n");
    expect(rewrite(lf)).toBe("`a\n\n[[New]] b` [[New]]");
    expect(rewrite(crlf)).toBe(rewrite(lf).replaceAll("\n", "\r\n"));
    // Negative control: without the blank line it is one code span.
    expect(rewrite("`a [[Old]] b` [[Old]]")).toBe("`a [[Old]] b` [[New]]");
  });

  it("stops rewriting once the rename window has passed", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(1_000_000);
      recordRenamedLinks("Old", "New");
      const linkRewrite = renameRewriteExplaining("[[Old]]", "[[New]]")!;
      expect(linkRewrite("[[Old]]")).toHaveLength(1);
      vi.setSystemTime(1_000_000 + 6 * 60_000);
      expect(linkRewrite("[[Old]]")).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does nothing for a title that cannot be written inside a link", () => {
    expect(rewrite("[[Old]]", "Old", "A|B")).toBe("[[Old]]");
  });

  it("explains a change only when a recent rename accounts for all of it", () => {
    const now = 1_000_000;
    recordRenamedLinks("Old", "New", now);
    expect(renameRewriteExplaining("See [[Old]].", "See [[New]].", now + 1000)).not.toBeNull();
    expect(renameRewriteExplaining("See [[Old]].", "See [[New]]. More", now + 1000)).toBeNull();
    expect(renameRewriteExplaining("See [[Old]].", "See [[Newer]].", now + 1000)).toBeNull();
    // An old rename no longer explains anything.
    expect(renameRewriteExplaining("See [[Old]].", "See [[New]].", now + 10 * 60_000)).toBeNull();
  });
});
