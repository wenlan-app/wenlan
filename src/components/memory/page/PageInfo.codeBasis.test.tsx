// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CodeBasisRef } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPages: vi.fn(),
  searchPages: vi.fn(),
  clipboardWrite: vi.fn(),
}));

import { clipboardWrite } from "../../../lib/tauri";
import { basisCountLabel } from "./format";
import PageInfo from "./PageInfo";
import i18n from "i18next";

const code = (over: Partial<CodeBasisRef> = {}): CodeBasisRef => ({
  repo_root: "/repo", path: "src/parser.rs", line_start: 10, line_end: 24, state: "ok", ...over,
});

function setup(props: Partial<React.ComponentProps<typeof PageInfo>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onUnmarkCodeBasis = vi.fn();
  render(<QueryClientProvider client={client}><PageInfo
    embedded
    sourceCount={3}
    sources={[]}
    inbound={[]}
    revisions={[]}
    citations={undefined}
    citationState="none"
    onMemoryClick={vi.fn()}
    currentPageId="page-1"
    currentSpace="Wenlan"
    basis={[]}
    onMarkBasis={vi.fn().mockResolvedValue(true)}
    onUnmarkBasis={vi.fn()}
    codeBasis={[]}
    onUnmarkCodeBasis={onUnmarkCodeBasis}
    {...props}
  /></QueryClientProvider>);
  return { onUnmarkCodeBasis };
}

afterEach(() => {
  cleanup();
  vi.mocked(clipboardWrite).mockReset();
});

describe("PageInfo code section", () => {
  it("has no code section for a note that relies on no code", () => {
    setup();
    expect(screen.queryByRole("region", { name: "Code this note relies on" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Code" })).toBeNull();
  });

  it("lists each code basis as path and line range with its state, after the pages section", () => {
    setup({
      codeBasis: [
        code(),
        code({ path: "src/lex.rs", line_start: 3, line_end: 3, state: "pending" }),
        code({ path: "src/gone.rs", line_start: 1, line_end: 2, state: "unavailable", reason: "file_missing" }),
        code({ path: "src/shifted.rs", line_start: 40, line_end: 50, state: "moved" }),
      ],
    });

    const section = screen.getByRole("region", { name: "Code this note relies on" });
    expect(within(section).getByRole("heading", { name: "Code" })).toBeInTheDocument();
    const rows = within(section).getAllByRole("listitem");
    expect(rows.map((item) => item.textContent?.replace(/\s+/g, " ").trim())).toEqual([
      "src/parser.rs:10\u201324Up to date",
      "src/lex.rs:3Changed",
      "src/gone.rs:1\u20132Not found",
      "src/shifted.rs:40\u201350Moved",
    ]);
    const path = rows[0].querySelector(".note-info-code-path");
    expect(path).toHaveTextContent("src/parser.rs:10\u201324");
    // The range is set in the UI face so the en dash does not read as a hyphen; the path stays mono.
    expect(path?.querySelector(".note-info-code-range")).toHaveTextContent(":10\u201324");
    expect(path?.firstChild?.textContent).toBe("src/parser.rs");
    expect(rows[1].querySelector(".note-info-code-range")).toBeNull();
    // After the pages section; there is no way to add code from here.
    const pages = screen.getByRole("region", { name: "Pages this page relies on" });
    expect(pages.compareDocumentPosition(section) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(section).queryByRole("button", { name: /add/i })).toBeNull();
  });

  it("copies the location (path and first line) when its row is clicked and says so briefly", async () => {
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    setup({ codeBasis: [code()] });

    fireEvent.click(screen.getByRole("button", { name: "src/parser.rs:10\u201324" }));

    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("src/parser.rs:10"));
    expect(await screen.findByText("Copied")).toBeInTheDocument();
    expect(screen.queryByText("Up to date")).toBeNull();
  });

  it("stops relying on a code basis from the overflow menu", () => {
    const { onUnmarkCodeBasis } = setup({ codeBasis: [code({ state: "pending" })] });

    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop relying on this code" }));

    expect(onUnmarkCodeBasis).toHaveBeenCalledWith(expect.objectContaining({ path: "src/parser.rs", line_start: 10 }));
  });
});

describe("basisCountLabel with code", () => {
  const t = i18n.t.bind(i18n);

  it("joins memories, pages and code, leaving out any part that is zero", () => {
    expect(basisCountLabel(t, 3, 2, 5)).toBe("3 source memories · 2 pages · 5 code references");
    expect(basisCountLabel(t, 3, 2)).toBe("3 source memories · 2 pages");
    expect(basisCountLabel(t, 0, 2)).toBe("2 pages");
    expect(basisCountLabel(t, 3, 0, 1)).toBe("3 source memories · 1 code reference");
    expect(basisCountLabel(t, 0, 0, 5)).toBe("5 code references");
  });
});
