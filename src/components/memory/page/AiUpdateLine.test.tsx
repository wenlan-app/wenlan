// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  revertPageSentences: vi.fn(),
}));

import { revertPageSentences } from "../../../lib/tauri";
import { AiUpdateLine, aiUpdateShows } from "./AiUpdateLine";

const update = { version: 7, changed_sentences: 3, at: 1_000 };

function renderLine(props: Partial<Parameters<typeof AiUpdateLine>[0]> = {}, wrap = (node: ReactNode) => node) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}>{wrap(
    <AiUpdateLine pageId="page-1" update={update} humanEditedAt={null} time="2h ago" disabled={false} {...props} />,
  )}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.mocked(revertPageSentences).mockReset();
});

describe("AI update line", () => {
  it("says how many sentences the AI updated and when", () => {
    renderLine();
    expect(screen.getByText("AI updated 3 sentences 2h ago")).toBeVisible();
    expect(screen.getByRole("button", { name: "Undo" })).toBeEnabled();
  });

  it("stays away once the person has edited the page since, or when nothing changed", () => {
    expect(aiUpdateShows(update, 999)).toBe(true);
    expect(aiUpdateShows(update, 1_000)).toBe(false);
    expect(aiUpdateShows(update, 2_000)).toBe(false);
    expect(aiUpdateShows({ ...update, changed_sentences: 0 }, null)).toBe(false);
    expect(aiUpdateShows(null, null)).toBe(false);
    renderLine({ humanEditedAt: 2_000 });
    expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
  });

  it("can't undo while the page is held by an edit or another action", () => {
    renderLine({ disabled: true });
    expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
  });

  it("undoes that one AI write and says what was put back", async () => {
    vi.mocked(revertPageSentences).mockResolvedValue({ page_id: "page-1", version: 8, reverted: 2, skipped: 1 });
    renderLine();
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Put back 2 sentences. 1 sentence changed again since, so it stays as it is.",
    );
    expect(revertPageSentences).toHaveBeenCalledWith("page-1", 7);
    expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
  });

  it("says so when every sentence changed again since", async () => {
    vi.mocked(revertPageSentences).mockResolvedValue({ page_id: "page-1", version: 7, reverted: 0, skipped: 3 });
    renderLine();
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Nothing to put back: those sentences changed again since.");
  });

  it("keeps Undo available after a failure", async () => {
    vi.mocked(revertPageSentences).mockRejectedValue(new Error("409"));
    renderLine();
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't undo the AI's update. Try again.");
    expect(screen.getByRole("button", { name: "Undo" })).toBeEnabled();
  });
});
