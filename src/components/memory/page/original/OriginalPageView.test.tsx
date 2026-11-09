// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { claudeSession, detailFor } from "../../sessions/testFixtures";
import { frameDocument, splitOriginal } from "./originalContent";

const mocks = vi.hoisted(() => ({
  getPageOriginal: vi.fn(),
  getPageSessions: vi.fn(),
  getSessionDetail: vi.fn(),
}));

vi.mock("../../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../../lib/tauri")>()),
  getPageOriginal: mocks.getPageOriginal,
  getPageSessions: mocks.getPageSessions,
  getSessionDetail: mocks.getSessionDetail,
}));

import { OriginalPageView } from "./OriginalPageView";

const HTML = "<!doctype html>\n<html><body><h1>Release map</h1></body></html>\n";
const SAVED = `Saved from a Claude Code session, 2026-11-08 18:30 UTC. Original file: \`originals/claude-code/s-1/a.html\`.\n\n\`\`\`html\n${HTML}\`\`\`\n`;

function renderView(content: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <OriginalPageView pageId="page_1" title="Release map" content={content}
        renderMarkdown={(markdown) => <div data-testid="markdown">{markdown}</div>} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.getPageSessions.mockResolvedValue([]);
  mocks.getPageOriginal.mockResolvedValue(null);
  mocks.getSessionDetail.mockImplementation(async () => detailFor(claudeSession));
});

describe("originals", () => {
  it("UX-ART-1 shows saved HTML exactly as it was, in a frame that cannot reach the app", () => {
    renderView(SAVED);
    const frame = screen.getByTestId("original-frame") as HTMLIFrameElement;
    // The original is all there, byte for byte, and only a fit script is added.
    const doc = frame.getAttribute("srcdoc") ?? "";
    expect(doc.replace(/<script>[\s\S]*?<\/script>/, "")).toBe(HTML);
    expect(frame.getAttribute("sandbox")).toContain("allow-scripts");
    expect(frame.getAttribute("sandbox")).not.toContain("allow-same-origin");
    // The file's first line is not shown as page text, and nothing is code.
    expect(screen.queryByText(/Saved from a Claude Code session/)).toBeNull();
    expect(screen.queryByTestId("markdown")).toBeNull();
  });

  it("UX-ART-2 names the conversation that kept it and when", async () => {
    mocks.getPageSessions.mockResolvedValue([{ session: claudeSession, via: "wrote" }]);
    renderView(SAVED);
    expect(await screen.findByRole("button", { name: "From Claude Code · Oct 20 · wenlan" })).toBeInTheDocument();
  });

  it("UX-ART-2 names the tool and day of an imported chat and opens it there", async () => {
    mocks.getPageOriginal.mockResolvedValue({
      page_id: "page_1", vendor: "claude", conversation_id: "c1",
      conversation_url: "https://claude.ai/chat/c1", artifact_id: "a", kind: "text/html",
      file_path: "originals/claude/c1/a.html", written_at: Date.UTC(2026, 9, 20, 9) / 1000, imported_at: 0,
    });
    renderView(SAVED.replace("Saved from a Claude Code session", "Kept from the Claude chat \"Plan\""));
    await waitFor(() => expect(screen.getByTestId("original-source")).toHaveTextContent("From Claude · Oct 20"));
    expect(screen.getByRole("button", { name: "Open in Claude" })).toBeInTheDocument();
  });

  it("UX-ART-3 shows notes added after the original below it", () => {
    renderView(`${SAVED}\nNote: the launch moved to May.\n`);
    expect(screen.getByTestId("original-frame")).toBeInTheDocument();
    expect(screen.getByTestId("original-notes")).toHaveTextContent("Note: the launch moved to May.");
  });

  it("shows a Markdown original as Markdown and code as code", () => {
    expect(splitOriginal("Kept from a Claude chat.\n\n# Plan\n\nShip it.\n")).toEqual({
      frame: null, body: "# Plan\n\nShip it.", notes: "",
    });
    expect(splitOriginal("Kept from a Claude chat.\n\n```python\nprint(1)\n```\n\nA note.")).toEqual({
      frame: null, body: "```python\nprint(1)\n```", notes: "A note.",
    });
  });

  it("puts an SVG on a page of its own and keeps the fit script inside the body", () => {
    const parts = splitOriginal("Saved by an AI tool, 2026-11-08 18:30 UTC.\n\n```svg\n<svg viewBox=\"0 0 1 1\"></svg>\n```\n");
    expect(parts.frame?.kind).toBe("svg");
    expect(frameDocument(parts.frame!)).toContain("<body><svg viewBox=\"0 0 1 1\"></svg>\n<script>");
    expect(frameDocument({ kind: "html", source: HTML })).toMatch(/<h1>Release map<\/h1><script>[\s\S]*<\/script><\/body>/);
  });
});
