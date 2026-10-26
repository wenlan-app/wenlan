// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, cleanup } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { Page } from "../../../lib/tauri";
import PageLinkPreview, { pageLinkPreviewExcerpt } from "./PageLinkPreview";

afterEach(() => { cleanup(); document.querySelectorAll("body > a").forEach((anchor) => anchor.remove()); vi.mocked(invoke).mockReset(); });

const page = {
  id: "target-1", title: "Target page", summary: null,
  content: "---\nprivate: [[Metadata]]\n---\n# Target page\n\nA useful Chinese preview 段落 with **plain text**.",
  entity_id: null, domain: "Wenlan", space: "Wenlan", source_memory_ids: [], version: 1,
  status: "active", creation_kind: "authored", review_status: "confirmed",
  created_at: "2026-10-07T00:00:00Z", last_compiled: "2026-10-07T00:00:00Z", last_modified: "2026-10-07T00:00:00Z", user_edited: false,
} as Page;

function renderPreview(onOpen = vi.fn(), response: Page | null = page) {
  vi.mocked(invoke).mockResolvedValue(response);
  const anchor = document.body.appendChild(document.createElement("a"));
  anchor.href = "#concept:target-1";
  anchor.textContent = "target";
  const onDismiss = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rendered = render(<QueryClientProvider client={client}><PageLinkPreview
    request={{ pageId: "target-1", anchor, keyboard: false }} onDismiss={onDismiss} onOpen={onOpen}
    onPointerEnter={vi.fn()} onPointerLeave={vi.fn()}
  /></QueryClientProvider>);
  return { ...rendered, onDismiss, onOpen, anchor };
}

describe("PageLinkPreview", () => {
  it("loads automatic target content and provides a distinct explicit Open action", async () => {
    const onOpen = vi.fn();
    const { anchor, onDismiss } = renderPreview(onOpen);
    const dialog = await screen.findByRole("dialog", { name: "Target page" }, { timeout: 2_000 });
    await waitFor(() => expect(dialog).toHaveTextContent("A useful Chinese preview 段落 with plain text."));
    expect(dialog).not.toHaveTextContent("private");
    expect(dialog).not.toHaveTextContent("# Target page");
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toContain("get_page");
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).not.toContain("get_page_explicit_browse");
    fireEvent.click(screen.getByRole("button", { name: "Open →" }));
    expect(onDismiss).toHaveBeenCalledOnce();
    expect(onOpen).toHaveBeenCalledWith("target-1");
    expect(anchor.isConnected).toBe(true);
  });

  it("renders an unavailable state when automatic scope returns no page", async () => {
    const { } = renderPreview(vi.fn(), null);
    expect(await screen.findByText("Preview unavailable.", {}, { timeout: 2_000 })).toBeInTheDocument();
  });

  it("shows loading and retryable errors, then keeps the requested target title", async () => {
    const anchor = document.body.appendChild(document.createElement("a"));
    anchor.dataset.wikiTargetLabel = "City walk";
    const onDismiss = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.mocked(invoke).mockRejectedValueOnce(new Error("offline"))
      .mockImplementationOnce(async () => page);
    render(<QueryClientProvider client={client}><PageLinkPreview
      request={{ pageId: "target-1", anchor, keyboard: true }} onDismiss={onDismiss} onOpen={vi.fn()}
      onPointerEnter={vi.fn()} onPointerLeave={vi.fn()}
    /></QueryClientProvider>);
    expect(await screen.findByRole("dialog", { name: "City walk" }, { timeout: 2_000 })).toBeVisible();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load this preview.");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(screen.getByRole("dialog", { name: "Target page" })).toBeVisible());
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual(["get_page", "get_page"]);
  });

  it("discards a preview when its source anchor is removed", async () => {
    const { anchor, onDismiss } = renderPreview();
    await screen.findByRole("dialog", { name: "Target page" }, { timeout: 2_000 });
    anchor.remove();
    await waitFor(() => expect(onDismiss).toHaveBeenCalled(), { timeout: 1_500 });
  });

  it("does not let a slow previous target replace the newly requested preview", async () => {
    const pending = new Map<string, (result: Page | null) => void>();
    vi.mocked(invoke).mockImplementation((_command, args) => new Promise((resolve) => {
      const id = (args as { id: string }).id;
      pending.set(id, resolve as (result: Page | null) => void);
    }));
    const firstAnchor = document.body.appendChild(document.createElement("a"));
    firstAnchor.dataset.wikiTargetLabel = "First";
    const secondAnchor = document.body.appendChild(document.createElement("a"));
    secondAnchor.dataset.wikiTargetLabel = "Second";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onDismiss = vi.fn();
    const props = (anchor: HTMLAnchorElement, pageId: string) => ({
      request: { pageId, anchor, keyboard: false }, onDismiss, onOpen: vi.fn(), onPointerEnter: vi.fn(), onPointerLeave: vi.fn(),
    });
    const view = render(<QueryClientProvider client={client}><PageLinkPreview {...props(firstAnchor, "first-id")} /></QueryClientProvider>);
    await waitFor(() => expect(pending.has("first-id")).toBe(true), { timeout: 2_000 });
    view.rerender(<QueryClientProvider client={client}><PageLinkPreview {...props(secondAnchor, "second-id")} /></QueryClientProvider>);
    await waitFor(() => expect(pending.has("second-id")).toBe(true), { timeout: 2_000 });
    pending.get("second-id")?.({ ...page, id: "second-id", title: "Second canonical" });
    expect(await screen.findByRole("dialog", { name: "Second canonical" })).toBeVisible();
    pending.get("first-id")?.({ ...page, id: "first-id", title: "Stale first" });
    await waitFor(() => expect(screen.getByRole("dialog", { name: "Second canonical" })).toBeVisible());
    expect(screen.queryByRole("dialog", { name: "Stale first" })).toBeNull();
  });

  it("strips frontmatter, duplicated title, and markdown punctuation from body excerpts", () => {
    expect(pageLinkPreviewExcerpt(page)).toBe("A useful Chinese preview 段落 with plain text.");
  });
});
