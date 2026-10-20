// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const previewFlushHarness = vi.hoisted(() => ({ flush: vi.fn<() => Promise<boolean>>() }));

vi.mock("../../../lib/tauri", () => ({
  acceptPendingRevision: vi.fn(),
  agentDisplayName: (slug: string | null) => slug,
  confirmSpace: vi.fn(),
  deleteFileChunks: vi.fn(),
  deleteSpace: vi.fn(),
  dismissPendingRevision: vi.fn(),
  FACET_COLORS: {},
  getNurtureCards: vi.fn().mockResolvedValue([]),
  getPendingRevision: vi.fn().mockResolvedValue(null),
  getSpace: vi.fn(),
  listIndexedFiles: vi.fn(),
  getVersionChain: vi.fn().mockResolvedValue([]),
  listEntities: vi.fn(),
  listMemoriesRich: vi.fn(),
  listPages: vi.fn(),
  pinMemory: vi.fn().mockResolvedValue(undefined),
  setStability: vi.fn(),
  STABILITY_TIERS: {},
  unpinMemory: vi.fn().mockResolvedValue(undefined),
  updateMemory: vi.fn(),
  updateSpace: vi.fn(),
}));

vi.mock("../sources/SourceAnnotations", async () => {
  const { useEffect } = await import("react");
  return { default: ({ onRegisterFlush }: { onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void }) => {
    useEffect(() => {
      onRegisterFlush?.(previewFlushHarness.flush);
      return () => onRegisterFlush?.(null);
    }, [onRegisterFlush]);
    return <section>Editable source description.</section>;
  } };
});

import {
  confirmSpace,
  deleteSpace,
  getSpace,
  listEntities,
  listIndexedFiles,
  listMemoriesRich,
  listPages,
  updateSpace,
} from "../../../lib/tauri";
import SpaceDetail from "../SpaceDetail";
import { SPACE_DETAIL_TEST_COPY } from "./testTranslation";

const space = {
  id: "s1",
  name: "Origin",
  description: "A working context",
  suggested: false,
  starred: false,
  sort_order: 0,
  memory_count: 1,
  entity_count: 1,
  created_at: 1_000,
  updated_at: 2_000,
};

const memory = {
  source_id: "m1",
  title: "Memory one",
  content: "Body",
  summary: null,
  memory_type: "fact",
  domain: "Origin",
  source_agent: "codex",
  confidence: 0.9,
  confirmed: true,
  pinned: false,
  supersedes: null,
  last_modified: 2_000,
  chunk_count: 1,
};

const entity = {
  id: "e1",
  name: "Lucian",
  entity_type: "person",
  domain: "Origin",
  source_agent: "codex",
  confidence: 0.8,
  confirmed: true,
  created_at: 1_000,
  updated_at: 2_000,
  memory_count: 1,
  status: "established" as const,
  established_by: "manual",
};

const page = {
  id: "p1",
  title: "Page one",
  summary: "Summary",
  content: "Content",
  entity_id: null,
  domain: "Origin",
  source_memory_ids: ["m1"],
  version: 1,
  status: "active",
  created_at: "2026-07-01T00:00:00Z",
  last_compiled: "2026-07-01T00:00:00Z",
  last_modified: "2026-07-02T00:00:00Z",
};

function renderDetail(overrides: Partial<React.ComponentProps<typeof SpaceDetail>> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  const props = {
    copy: SPACE_DETAIL_TEST_COPY,
    spaceName: "Origin",
    onBack: vi.fn(),
    onSelectMemory: vi.fn(),
    onSelectPage: vi.fn(),
    onEntityClick: vi.fn(),
    ...overrides,
  };
  const result = render(
    <QueryClientProvider client={client}>
      <SpaceDetail {...props} />
    </QueryClientProvider>,
  );
  return { ...result, props };
}

describe("SpaceDetail existing behavior", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getSpace).mockResolvedValue(space);
    vi.mocked(listMemoriesRich).mockResolvedValue([memory]);
    vi.mocked(listEntities).mockResolvedValue([entity]);
    vi.mocked(listPages).mockResolvedValue([page]);
    previewFlushHarness.flush.mockReset();
    previewFlushHarness.flush.mockResolvedValue(true);
    vi.mocked(listIndexedFiles).mockResolvedValue([{
      source: "webpage", source_id: "https://example.com/article", title: "Source article",
      chunk_count: 0, last_modified: 1, url: "https://example.com/article",
    }]);
  });

  it("keeps a suggested space through confirmSpace", async () => {
    vi.mocked(getSpace).mockResolvedValue({ ...space, suggested: true });
    renderDetail();

    fireEvent.click(await screen.findByRole("button", { name: "Keep" }));

    await waitFor(() => expect(confirmSpace).toHaveBeenCalledWith("Origin"));
  });

  it("renames the title on Enter", async () => {
    renderDetail();
    await screen.findByRole("textbox", { name: "Name" });
    const input = screen.getByDisplayValue("Origin");

    fireEvent.change(input, { target: { value: "Planning" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() =>
      expect(updateSpace).toHaveBeenCalledWith("Origin", "Planning", "A working context"),
    );
  });

  it("reports a successful load with the stable Space identity", async () => {
    // Given an integrated successful-load callback
    const onSpaceLoaded = vi.fn();

    // When the dossier query resolves
    renderDetail({ onSpaceLoaded });

    // Then the loaded Space is reported once
    await waitFor(() => expect(onSpaceLoaded).toHaveBeenCalledWith(space));
    expect(onSpaceLoaded).toHaveBeenCalledTimes(1);
  });

  it("reports successful rename and delete mutations by stable id", async () => {
    // Given integrated MRU cleanup callbacks
    const onSpaceRenamed = vi.fn();
    const onSpaceDeleted = vi.fn();
    vi.mocked(updateSpace).mockResolvedValue({ ...space, name: "Planning" });
    vi.mocked(deleteSpace).mockResolvedValue(undefined);
    const onBack = vi.fn();
    const first = renderDetail({ onSpaceRenamed, onBack });

    // When rename succeeds
    await screen.findByRole("textbox", { name: "Name" });
    fireEvent.change(screen.getByDisplayValue("Origin"), { target: { value: "Planning" } });
    fireEvent.keyDown(screen.getByDisplayValue("Planning"), { key: "Enter" });

    // Then the stable id and new name are reported
    await waitFor(() => expect(onSpaceRenamed).toHaveBeenCalledWith({ id: "s1", name: "Planning" }));
    expect(onBack).not.toHaveBeenCalled();
    first.unmount();

    // When delete succeeds on a fresh dossier
    const deleted = renderDetail({ onSpaceDeleted });
    fireEvent.click(await screen.findByRole("button", { name: "Actions for Origin" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete space" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete space" }));

    // Then the deleted stable id is reported
    await waitFor(() => expect(onSpaceDeleted).toHaveBeenCalledWith("s1"));
    expect(deleted.props.onBack).not.toHaveBeenCalled();
  });

  it("waits for a source preview save before confirmed deletion and keeps the preview on failure", async () => {
    let settle!: (saved: boolean) => void;
    previewFlushHarness.flush.mockImplementationOnce(() => new Promise((resolve) => { settle = resolve; }));
    vi.mocked(deleteSpace).mockResolvedValue(undefined);
    const onSpaceDeleted = vi.fn();
    const onBack = vi.fn();
    renderDetail({ onSpaceDeleted, onBack });

    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    fireEvent.click(await screen.findByRole("button", { name: /Source article/ }));
    expect(screen.getByText("Editable source description.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Actions for Origin" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete space" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete space" }));

    await waitFor(() => expect(previewFlushHarness.flush).toHaveBeenCalledOnce());
    expect(deleteSpace).not.toHaveBeenCalled();
    await act(async () => { settle(false); });
    expect(deleteSpace).not.toHaveBeenCalled();
    expect(onSpaceDeleted).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
    expect(screen.getByText("Editable source description.")).toBeInTheDocument();

    previewFlushHarness.flush.mockResolvedValueOnce(true);
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete space" }));
    await waitFor(() => expect(deleteSpace).toHaveBeenCalledWith("Origin"));
    await waitFor(() => expect(onSpaceDeleted).toHaveBeenCalledWith("s1"));
    expect(onBack).not.toHaveBeenCalled();
  });

  it("edits the description on Cmd+Enter", async () => {
    renderDetail();
    await screen.findByRole("textbox", { name: "Name" });
    const textarea = screen.getByRole("textbox", { name: "Edit description" });

    fireEvent.change(textarea, { target: { value: "New context" } });
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });

    await waitFor(() =>
      expect(updateSpace).toHaveBeenCalledWith("Origin", "Origin", "New context"),
    );
  });

  it("opens a page row", async () => {
    const onSelectPage = vi.fn();
    renderDetail({ onSelectPage });

    fireEvent.click(await screen.findByRole("button", { name: /Page one/ }));

    expect(onSelectPage).toHaveBeenCalledWith("p1");
  });

  it("keeps memories and topics out of the Space workspace", async () => {
    renderDetail();
    await screen.findByRole("tab", { name: "Pages" });
    expect(screen.queryByText("Key entities")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Raw memories (1)" })).not.toBeInTheDocument();
    expect(listEntities).not.toHaveBeenCalled();
    expect(listMemoriesRich).not.toHaveBeenCalled();
  });

  it("renders distinct loading, error, and missing-space states", async () => {
    vi.mocked(getSpace).mockImplementation(() => new Promise(() => {}));
    const loading = renderDetail();
    expect(screen.getByRole("status")).toHaveTextContent("Loading space");
    loading.unmount();

    vi.mocked(getSpace).mockRejectedValue(new Error("offline"));
    const failed = renderDetail();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not open space");
    failed.unmount();

    vi.mocked(getSpace).mockResolvedValue(null);
    renderDetail();
    expect(await screen.findByText("Space not found")).toBeInTheDocument();
  });
});
