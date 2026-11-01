// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { flagPageSentence, recheckPageSentence, type PageSentence } from "../../../lib/tauri";
import { ThisSentence } from "./ThisSentence";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  flagPageSentence: vi.fn(),
  recheckPageSentence: vi.fn(),
}));

function sentence(overrides: Partial<PageSentence> = {}): PageSentence {
  return {
    idx: 0,
    text: "Tram 28 runs all night.",
    hash: "h3",
    author: "ai",
    protected: false,
    evidence: "unverified",
    weak: ["stale"],
    ...overrides,
  };
}

function renderIt(value: PageSentence | null, onFixMyself = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <ThisSentence pageId="page-1" sentence={value} onFixMyself={onFixMyself} disabled={false} />
    </QueryClientProvider>,
  );
  return { onFixMyself };
}

describe("ThisSentence", () => {
  beforeEach(() => {
    vi.mocked(flagPageSentence).mockReset().mockResolvedValue({ page_id: "page-1", version: 2, sentence_hash: "h3", seq: 1 });
    vi.mocked(recheckPageSentence).mockReset().mockResolvedValue({ page_id: "page-1", version: 2, sentence: sentence({ weak: [] }) });
  });

  it("shows nothing without a sentence under the cursor", () => {
    renderIt(null);
    expect(screen.queryByTestId("this-sentence")).toBeNull();
  });

  it("UX-WHO-1: says who wrote it, counting an accepted sentence as yours", () => {
    renderIt(sentence({ author: "ai", protected: true, accepted: true, weak: [] }));
    expect(screen.getByText("Yours, you accepted it")).toBeTruthy();
  });

  it("UX-WHO-4: rechecks only this sentence", async () => {
    renderIt(sentence());
    expect(screen.getByText("Can’t be backed up: its source changed since")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ask AI to recheck" }));
    await waitFor(() => expect(recheckPageSentence).toHaveBeenCalledWith("page-1", "h3"));
  });

  it("UX-WHO-3: 「不對」 lets the person fix it themselves", () => {
    const { onFixMyself } = renderIt(sentence());
    fireEvent.click(screen.getByRole("button", { name: "Not right" }));
    fireEvent.click(screen.getByRole("button", { name: "I’ll fix it" }));
    expect(onFixMyself).toHaveBeenCalledWith("Tram 28 runs all night.");
    expect(flagPageSentence).not.toHaveBeenCalled();
  });

  it("UX-WHO-3: 「不對」 asks the AI, whose fix is still a proposal", async () => {
    vi.mocked(flagPageSentence).mockResolvedValue({ page_id: "page-1", version: 2, sentence_hash: "h3", seq: 1, ai_fix_queued: true });
    renderIt(sentence());
    fireEvent.click(screen.getByRole("button", { name: "Not right" }));
    fireEvent.click(screen.getByRole("button", { name: "Let AI fix it" }));
    await waitFor(() => expect(flagPageSentence).toHaveBeenCalledWith("page-1", "h3", true));
    expect(await screen.findByText(/Nothing changes until you accept/)).toBeTruthy();
  });

  it("UX-WHO-3: says so when no AI model can fix it", async () => {
    renderIt(sentence());
    fireEvent.click(screen.getByRole("button", { name: "Not right" }));
    fireEvent.click(screen.getByRole("button", { name: "Let AI fix it" }));
    expect(await screen.findByText("No AI model is set up to fix it. You can fix it yourself.")).toBeTruthy();
  });
});
