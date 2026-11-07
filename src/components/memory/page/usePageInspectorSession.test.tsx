// SPDX-License-Identifier: AGPL-3.0-only
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { usePageInspectorSession } from "./usePageInspectorSession";

function wrapper(client = new QueryClient()) {
  return ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("per-page inspector session", () => {
  it("restores each page immediately on switch and across remounts", () => {
    const provider = wrapper();
    const first = renderHook(({ id }) => usePageInspectorSession(id), { initialProps: { id: "a" }, wrapper: provider });
    act(() => {
      first.result.current.setPdfPreview({ source: "file", source_agent: "zotero", source_id: "paper-a", title: "Paper A", chunk_count: 1, last_modified: 0 });
      first.result.current.setPdfPreviewOpen(true);
    });
    first.rerender({ id: "b" });
    expect(first.result.current.pdfPreview).toBeNull();
    expect(first.result.current.pdfPreviewOpen).toBe(false);
    act(() => first.result.current.setInfoOpen(true));
    first.rerender({ id: "a" });
    expect(first.result.current.pdfPreview?.source_id).toBe("paper-a");
    expect(first.result.current.pdfPreviewOpen).toBe(true);
    expect(first.result.current.infoOpen).toBe(false);
    act(() => first.result.current.setPdfPreviewOpen(false));
    first.unmount();
    const reopened = renderHook(() => usePageInspectorSession("a"), { wrapper: provider });
    expect(reopened.result.current.pdfPreview?.source_id).toBe("paper-a");
    expect(reopened.result.current.pdfPreviewOpen).toBe(false);
  });

  it("isolates independent application clients", () => {
    const first = renderHook(() => usePageInspectorSession("same-page"), { wrapper: wrapper() });
    act(() => first.result.current.setInfoOpen(true));
    const second = renderHook(() => usePageInspectorSession("same-page"), { wrapper: wrapper() });
    expect(second.result.current.infoOpen).toBe(false);
  });
});
