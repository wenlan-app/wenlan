// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import type { SearchResult } from "../../lib/tauri";

import MemorySearchResult from "./MemorySearchResult";

function makeResult(overrides: Partial<SearchResult> = {}): SearchResult {
  return {
    id: "row-1",
    content: "User: I prefer tea\n\nAssistant: Noted.",
    source: "memory",
    source_id: "import_claude_c1",
    title: "Tea",
    url: null,
    chunk_index: 0,
    last_modified: 1_700_000_000,
    score: 0.5,
    memory_type: "preference",
    ...overrides,
  };
}

describe("MemorySearchResult", () => {
  it("UX-ADD-4 labels an imported chat turn as a chat transcript, never by its stored type", () => {
    const { rerender } = render(
      <MemorySearchResult result={makeResult({ semantic_unit: "chat_turn" })} query="tea" />,
    );
    expect(screen.getByTestId("search-result-conversation-text")).toHaveTextContent("Chat transcript");
    expect(screen.queryByText("preference")).not.toBeInTheDocument();

    // A person's own memory still shows its type.
    rerender(<MemorySearchResult result={makeResult({ semantic_unit: undefined })} query="tea" />);
    expect(screen.queryByTestId("search-result-conversation-text")).toBeNull();
    expect(screen.getByText("preference")).toBeInTheDocument();
  });
});
