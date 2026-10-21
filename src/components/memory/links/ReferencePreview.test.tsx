// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import type { MemoryItem, PageCitation } from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import { ReferencePreview } from "./ReferencePreview";

afterEach(() => { cleanup(); vi.mocked(invoke).mockReset(); });

const citation: PageCitation = {
  occurrence: 1,
  marker: 1,
  source_kind: "external_file",
  locator: "source-1::/notes/original.md",
  score: 0.8,
  status: "unverified",
  scope: "sentence",
};

const sourceMemory: MemoryItem = {
  source_id: "source-1",
  title: "Imported meeting notes",
  content: "---\nprivate: metadata\n---\n# Imported meeting notes\n\nThe source body has a useful plain-text excerpt.",
  summary: null,
  memory_type: "source",
  domain: null,
  source_agent: null,
  confidence: null,
  confirmed: true,
  pinned: false,
  supersedes: null,
  last_modified: 1,
  chunk_count: 1,
};

describe("ReferencePreview citation targets", () => {
  it("uses supplied source content without lookup and keeps the original action explicit", async () => {
    const anchor = document.body.appendChild(document.createElement("a"));
    anchor.textContent = "original.md";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}>
      <ReferencePreview
        request={{ target: { kind: "citation", citation, sourceMemory, sourcesLoading: false }, anchor, keyboard: false }}
        onDismiss={vi.fn()}
        onOpenPage={vi.fn()}
        onOpenMemory={vi.fn()}
        onPointerEnter={vi.fn()}
        onPointerLeave={vi.fn()}
      />
    </QueryClientProvider>);

    const preview = screen.getByRole("dialog", { name: "Imported meeting notes" });
    expect(preview).toHaveTextContent("File");
    expect(preview).toHaveTextContent("unverified");
    expect(preview).toHaveTextContent("The source body has a useful plain-text excerpt.");
    expect(preview).toHaveTextContent("/notes/original.md");
    expect(vi.mocked(invoke)).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /^Open file/ }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("open_file", { path: "/notes/original.md" });
  });

  it("does not show a delayed open failure after switching to another target and anchor", async () => {
    let rejectOpen!: (reason: Error) => void;
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === "open_file") return new Promise((_resolve, reject) => { rejectOpen = reject; }) as never;
      return Promise.resolve(null) as never;
    });
    const anchorA = document.body.appendChild(document.createElement("a"));
    anchorA.textContent = "original-a.md";
    const anchorB = document.body.appendChild(document.createElement("a"));
    anchorB.textContent = "original-b.md";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const renderPreview = (target: PageCitation, anchor: HTMLAnchorElement) => <QueryClientProvider client={client}>
      <ReferencePreview
        request={{ target: { kind: "citation", citation: target, sourceMemory: null, sourcesLoading: false }, anchor, keyboard: false }}
        onDismiss={vi.fn()}
        onPointerEnter={vi.fn()}
        onPointerLeave={vi.fn()}
      />
    </QueryClientProvider>;
    const view = render(renderPreview(citation, anchorA));

    fireEvent.click(screen.getByRole("button", { name: /^Open file/ }));
    await waitFor(() => expect(rejectOpen).toBeTypeOf("function"));
    view.rerender(renderPreview(citation, anchorB));
    await act(async () => { rejectOpen(new Error("old target could not open")); });

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "File · original.md" })).toBeInTheDocument();
  });
});

describe("ReferencePreview saved app links", () => {
  const gmail: PageCitation = {
    ...citation,
    source_kind: "external_url",
    locator: "https://mail.google.com/mail/u/0/#inbox/FMfcgzQ",
    status: "verified",
  };
  const renderPreview = (target: PageCitation, onOpenCitationTarget = vi.fn()) => {
    const anchor = document.body.appendChild(document.createElement("a"));
    anchor.textContent = "Gmail";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}>
      <ReferencePreview
        request={{ target: { kind: "citation", citation: target, sourceMemory: null, sourcesLoading: false }, anchor, keyboard: false }}
        onDismiss={vi.fn()}
        onOpenCitationTarget={onOpenCitationTarget}
        onPointerEnter={vi.fn()}
        onPointerLeave={vi.fn()}
      />
    </QueryClientProvider>);
    return onOpenCitationTarget;
  };

  it.each([
    ["en", "Open in Gmail →"],
    ["zh-Hans", "在 Gmail 打开 →"],
    ["zh-Hant", "在 Gmail 開啟 →"],
  ])("names the app and offers to open it in that app in %s", async (lng, action) => {
    await i18n.changeLanguage(lng);
    try {
      const onOpen = renderPreview(gmail);
      const preview = screen.getByRole("dialog");
      expect(preview).toHaveTextContent("Gmail");
      expect(preview.querySelector('[data-resource-icon="gmail"]')).not.toBeNull();
      expect(preview).toHaveTextContent(gmail.locator);
      fireEvent.click(screen.getByRole("button", { name: action }));
      expect(onOpen).toHaveBeenCalledWith(gmail);
    } finally {
      await i18n.changeLanguage("en");
    }
  });

  it("pins the open action to the bottom of the card so long content cannot scroll it away", () => {
    renderPreview(gmail);
    const actions = screen.getByRole("button", { name: "Open in Gmail →" }).closest<HTMLElement>('[data-reference-preview-actions="true"]');
    expect(actions).not.toBeNull();
    expect(actions!.style.position).toBe("sticky");
    expect(screen.getByRole("dialog").lastElementChild).toBe(actions);
  });

  it("keeps the browser wording for a site that is not a known app", () => {
    renderPreview({ ...gmail, locator: "https://example.com/blog" });
    expect(screen.getByRole("button", { name: "Open in browser →" })).toBeInTheDocument();
    expect(screen.getByRole("dialog").querySelector('[data-resource-icon="webpage"]')).not.toBeNull();
  });
});
