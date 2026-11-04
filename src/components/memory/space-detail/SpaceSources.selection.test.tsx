// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { listAllTags, listIndexedFiles } from "../../../lib/tauri";
import { SpaceSources } from "./SpaceSources";

vi.mock("../../../lib/tauri", () => ({ listIndexedFiles: vi.fn(), listAllTags: vi.fn() }));
vi.mock("../sources/SourceDocumentPreview", () => ({ default: () => null }));

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(listIndexedFiles).mockResolvedValue([{ source: "webpage", source_id: "webpage::one", title: "One article", chunk_count: 1, last_modified: 0 }]);
  vi.mocked(listAllTags).mockResolvedValue({ tags: ["research", "archived"], categories: [], document_categories: {}, document_tags: { "webpage::webpage::one": ["research"] } });
  sessionStorage.clear();
});

describe("SpaceSources selection", () => {
  it("starts with no source selected and shows batch actions only after selection", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><SpaceSources spaceId="s" spaceName="Research" lens="rows" /></QueryClientProvider>);
    expect(await screen.findByRole("button", { name: "Select" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Summarize & suggest classifications/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Select" }));
    expect(screen.getByRole("checkbox", { name: "Select One article" })).not.toBeChecked();
    expect(screen.queryByRole("button", { name: /Edit tags/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Select One article" }));
    expect(screen.getByRole("button", { name: "Summarize & suggest classifications… (1)" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit tags… (1)" })).toBeInTheDocument();
    expect(screen.getAllByText("research").length).toBeGreaterThan(1);
    expect(screen.getByLabelText("Filter by tag")).toHaveValue("");
  });

  it("restores selected sources after a Space view unmount and filters rows by tag", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    function Host() {
      const [active, setActive] = React.useState(true);
      return <><button onClick={() => setActive((value) => !value)}>Switch tab</button>{active ? <SpaceSources spaceId="s" spaceName="Research" lens="rows" /> : <p>Other tab</p>}</>;
    }
    render(<QueryClientProvider client={client}><Host /></QueryClientProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "Select" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select One article" }));
    fireEvent.click(screen.getByRole("button", { name: "Switch tab" }));
    fireEvent.click(screen.getByRole("button", { name: "Switch tab" }));
    fireEvent.click(await screen.findByRole("button", { name: "Select" }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "Select One article" })).toBeChecked());
    fireEvent.change(screen.getByLabelText("Filter by tag"), { target: { value: "research" } });
    expect(screen.getByText("One article")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Filter by tag"), { target: { value: "archived" } });
    expect(screen.getByText("No sources match this tag.")).toBeInTheDocument();
  });
});
