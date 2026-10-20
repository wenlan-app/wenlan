import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  createPageDraft,
  getPage,
  publishPageDraft,
  updatePageDraft,
  type Page,
} from "../../../lib/tauri";
import {
  PageDraftEditor,
  type PageDraftEditorHandle,
} from "./PageDraftEditor";

vi.mock("../../../lib/tauri", () => ({
  createPageDraft: vi.fn(),
  discardPageDraft: vi.fn(),
  getPage: vi.fn(),
  publishPageDraft: vi.fn(),
  updatePageDraft: vi.fn(),
}));

function page(overrides: Partial<Page> = {}): Page {
  return {
    id: "draft-1",
    title: "Draft title",
    summary: null,
    content: "Draft body",
    entity_id: null,
    domain: null,
    space: null,
    source_memory_ids: [],
    version: 3,
    status: "draft",
    creation_kind: "authored",
    review_status: "unconfirmed",
    created_at: "2026-07-16T00:00:00Z",
    last_compiled: "2026-07-16T00:00:00Z",
    last_modified: "2026-07-16T00:00:00Z",
    ...overrides,
  };
}

function renderEditor(props: Partial<React.ComponentProps<typeof PageDraftEditor>> = {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onBack = vi.fn();
  const onPublished = vi.fn();
  const onOpenExisting = vi.fn();
  const result = render(
    <QueryClientProvider client={queryClient}>
      <PageDraftEditor
        onBack={onBack}
        onOpenExisting={onOpenExisting}
        onPublished={onPublished}
        space={null}
        {...props}
      />
    </QueryClientProvider>,
  );
  return { ...result, onBack, onOpenExisting, onPublished, queryClient };
}

describe("PageDraftEditor", () => {
  beforeEach(() => {
    vi.mocked(createPageDraft).mockReset();
    vi.mocked(getPage).mockReset();
    vi.mocked(publishPageDraft).mockReset();
    vi.mocked(updatePageDraft).mockReset();
  });

  it("has no manual Publish bar or Space selector and finalizes through flush", async () => {
    vi.mocked(createPageDraft).mockResolvedValue(page({ version: 1, title: "Quick page", content: "" }));
    vi.mocked(publishPageDraft).mockResolvedValue(page({ title: "Quick page", status: "active", version: 2 }));
    const ref = createRef<PageDraftEditorHandle>();
    const onDraftIdentity = vi.fn();
    const { onPublished } = renderEditor({ ref, space: "Work", onDraftIdentity });

    const title = await screen.findByRole("textbox", { name: "Title" });
    const content = screen.getByRole("textbox", { name: "Content" });
    expect(title).toHaveFocus();
    expect(screen.queryByRole("button", { name: /publish/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Space" })).not.toBeInTheDocument();
    await userEvent.type(title, "Quick page");

    const handle = ref.current!;
    await act(async () => expect(await handle.flush()).toBe(true));

    expect(createPageDraft).toHaveBeenCalledWith(expect.objectContaining({
      title: "Quick page",
      content: "",
      space: "Work",
    }));
    expect(publishPageDraft).toHaveBeenCalledWith({ id: "draft-1", expectedVersion: 1 });
    expect(onDraftIdentity).toHaveBeenCalledWith("draft-1");
    expect(screen.getByRole("textbox", { name: "Title" })).toBe(title);
    expect(screen.getByRole("textbox", { name: "Content" })).toBe(content);
    expect(handle.getIdentity().publishedPage).toMatchObject({ id: "draft-1", status: "active" });
    expect(onPublished).not.toHaveBeenCalled();
  });

  it.each(["loading", "error", "missing"] as const)(
    "keeps a %s draft state reachable through Back without a hydrated editor",
    async (state) => {
      if (state === "loading") {
        vi.mocked(getPage).mockReturnValue(new Promise(() => {}));
      } else if (state === "error") {
        vi.mocked(getPage).mockRejectedValue(new Error("offline"));
      } else {
        vi.mocked(getPage).mockResolvedValue(null as unknown as Page);
      }
      const { onBack } = renderEditor({ draftId: "missing-draft" });
      if (state === "loading") await screen.findByText("Loading page…");
      if (state === "error") await screen.findByRole("alert");
      if (state === "missing") await screen.findByText("This page no longer exists.");

      await userEvent.click(screen.getByRole("button", { name: "Back" }));
      expect(onBack).toHaveBeenCalledOnce();
    },
  );

  it("keeps the existing Page detail object-title typography", () => {
    const css = readFileSync(resolve("src/components/memory/pages/pageDraftEditor.css"), "utf8");
    const titleRule = css.match(/\.page-draft-title\s*\{(?<body>[^}]*)\}/)?.groups?.body;

    expect(titleRule).toContain("font-family: var(--mem-font-heading);");
    expect(titleRule).toContain("font-size: clamp(24px, 1.6vw + 14px, 30px);");
    expect(titleRule).toContain("font-weight: 500;");
    expect(titleRule).toContain("line-height: 1.22;");
    expect(titleRule).toContain("letter-spacing: -0.01em;");
  });

  it.each([
    ["title-only", "A title", ""],
    ["body-only", "", "A first line\nMore body"],
  ])("finalizes %s content on flush", async (_name, titleValue, bodyValue) => {
    vi.mocked(createPageDraft).mockResolvedValue(page({
      title: titleValue,
      content: bodyValue,
      version: 1,
    }));
    vi.mocked(publishPageDraft).mockResolvedValue(page({
      title: titleValue,
      content: bodyValue,
      status: "active",
      version: 2,
    }));
    const ref = createRef<PageDraftEditorHandle>();
    renderEditor({ ref });
    fireEvent.change(await screen.findByRole("textbox", { name: "Title" }), { target: { value: titleValue } });
    fireEvent.change(screen.getByRole("textbox", { name: "Content" }), { target: { value: bodyValue } });

    await act(async () => expect(await ref.current!.flush()).toBe(true));

    expect(publishPageDraft).toHaveBeenCalledTimes(1);
  });

  it("does not create or publish an exactly empty page", async () => {
    const ref = createRef<PageDraftEditorHandle>();
    const { onBack } = renderEditor({ ref });

    await act(async () => expect(await ref.current!.flush()).toBe(true));
    await act(async () => expect(await ref.current!.requestBack()).toBeUndefined());

    expect(createPageDraft).not.toHaveBeenCalled();
    expect(publishPageDraft).not.toHaveBeenCalled();
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("waits for durable autosave and finalization before requesting navigation", async () => {
    let resolveCreate!: (value: Page) => void;
    vi.mocked(createPageDraft).mockReturnValue(new Promise((resolve) => {
      resolveCreate = resolve;
    }));
    vi.mocked(publishPageDraft).mockResolvedValue(page({ title: "A page", content: "", status: "active", version: 2 }));
    const ref = createRef<PageDraftEditorHandle>();
    const { onBack } = renderEditor({ ref });
    fireEvent.change(await screen.findByRole("textbox", { name: "Title" }), { target: { value: "A page" } });

    let leaving!: Promise<void>;
    act(() => { leaving = ref.current!.requestBack(); });
    expect(onBack).not.toHaveBeenCalled();
    expect(publishPageDraft).not.toHaveBeenCalled();

    await act(async () => {
      resolveCreate(page({ title: "A page", content: "", version: 1 }));
      await leaving;
    });
    expect(publishPageDraft).toHaveBeenCalledWith({ id: "draft-1", expectedVersion: 1 });
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("coalesces repeated flushes and leaves navigation to the caller", async () => {
    vi.mocked(getPage).mockResolvedValue(page());
    vi.mocked(publishPageDraft).mockResolvedValue(page({ status: "active", version: 4 }));
    const ref = createRef<PageDraftEditorHandle>();
    const { onBack, onPublished } = renderEditor({ draftId: "draft-1", ref });
    await screen.findByRole("textbox", { name: "Title" });

    await act(async () => {
      expect(await ref.current!.flush()).toBe(true);
      expect(await ref.current!.flush()).toBe(true);
    });

    expect(publishPageDraft).toHaveBeenCalledTimes(1);
    expect(ref.current!.getIdentity().publishedPage).toMatchObject({ status: "active" });
    expect(onBack).not.toHaveBeenCalled();
    expect(onPublished).not.toHaveBeenCalled();
  });

  it("preserves the page after a save failure and retries finalization explicitly", async () => {
    vi.mocked(createPageDraft)
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(page({ title: "Keep me", content: "", version: 1 }));
    vi.mocked(publishPageDraft).mockResolvedValue(page({ title: "Keep me", content: "", status: "active", version: 2 }));
    const ref = createRef<PageDraftEditorHandle>();
    renderEditor({ ref });
    const title = await screen.findByRole("textbox", { name: "Title" });
    fireEvent.change(title, { target: { value: "Keep me" } });

    await act(async () => expect(await ref.current!.flush()).toBe(false));
    expect(title).toHaveValue("Keep me");
    expect(await screen.findByRole("alert")).toHaveTextContent("Page couldn't be saved.");

    await userEvent.click(screen.getByRole("button", { name: "Retry save" }));
    await waitFor(() => expect(publishPageDraft).toHaveBeenCalledTimes(1));
    expect(title).toHaveValue("Keep me");
  });

  it("keeps an unknown publish locked and reconciles active state before retrying autosave", async () => {
    const active = page({ status: "active", version: 4 });
    vi.mocked(getPage)
      .mockResolvedValueOnce(page())
      .mockRejectedValueOnce(new Error("readback offline"))
      .mockResolvedValueOnce(active);
    vi.mocked(publishPageDraft).mockRejectedValueOnce(new Error("response lost"));
    const ref = createRef<PageDraftEditorHandle>();
    const { onPublished } = renderEditor({ draftId: "draft-1", ref });
    const title = await screen.findByRole("textbox", { name: "Title" });

    await act(async () => expect(await ref.current!.flush()).toBe(false));
    expect(title).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Retry save" }));

    await waitFor(() => expect(onPublished).toHaveBeenCalledWith("draft-1", undefined));
    expect(updatePageDraft).not.toHaveBeenCalled();
    expect(publishPageDraft).toHaveBeenCalledTimes(1);
  });

  it("unlocks an uncertain page only after readback confirms the unchanged draft", async () => {
    vi.mocked(getPage)
      .mockResolvedValueOnce(page())
      .mockRejectedValueOnce(new Error("first readback offline"))
      .mockResolvedValueOnce(page())
      .mockResolvedValueOnce(page());
    vi.mocked(publishPageDraft)
      .mockRejectedValueOnce(new Error("first response lost"))
      .mockRejectedValueOnce(new Error("second response lost"));
    const ref = createRef<PageDraftEditorHandle>();
    renderEditor({ draftId: "draft-1", ref });
    const title = await screen.findByRole("textbox", { name: "Title" });

    await act(async () => expect(await ref.current!.flush()).toBe(false));
    expect(title).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Retry save" }));

    await waitFor(() => expect(title).toBeEnabled());
    expect(title).toHaveValue("Draft title");
    expect(publishPageDraft).toHaveBeenCalledTimes(2);
    expect(updatePageDraft).not.toHaveBeenCalled();
  });

  it("blocks a newer remote draft without replacing the current editor text", async () => {
    const changed = Object.assign(new Error("stale"), { code: "draft_version_conflict" });
    vi.mocked(getPage).mockResolvedValueOnce(page());
    vi.mocked(updatePageDraft).mockResolvedValueOnce(page({ title: "My title", version: 4 }));
    vi.mocked(publishPageDraft).mockRejectedValueOnce(changed);
    vi.mocked(getPage).mockResolvedValueOnce(page({ title: "Remote title", version: 5 }));
    const ref = createRef<PageDraftEditorHandle>();
    renderEditor({ draftId: "draft-1", ref });
    const title = await screen.findByRole("textbox", { name: "Title" });
    fireEvent.change(title, { target: { value: "My title" } });

    await act(async () => expect(await ref.current!.flush()).toBe(false));

    expect(title).toHaveValue("My title");
    expect(await screen.findByRole("alert")).toHaveTextContent("This page changed elsewhere.");
    expect(screen.getByRole("button", { name: "Reload latest" })).toBeInTheDocument();
  });

  it("keeps exact-title conflicts on the saved page and offers rename only", async () => {
    const conflict = Object.assign(new Error("title conflict"), {
      code: "page_title_conflict",
      existingPageId: "page-existing",
      existingPageTitle: "Existing",
    });
    vi.mocked(getPage).mockResolvedValue(page());
    vi.mocked(publishPageDraft).mockRejectedValueOnce(conflict);
    const ref = createRef<PageDraftEditorHandle>();
    const { onOpenExisting } = renderEditor({ draftId: "draft-1", ref });
    const title = await screen.findByRole("textbox", { name: "Title" });

    await act(async () => expect(await ref.current!.flush()).toBe(false));
    expect(await screen.findByRole("alert")).toHaveTextContent("A page with this title already exists.");
    expect(title).toHaveValue("Draft title");
    expect(screen.queryByRole("button", { name: "Open existing" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Rename page" }));
    expect(title).toHaveFocus();
    expect(title).toHaveSelection();
    expect(onOpenExisting).not.toHaveBeenCalled();
    expect(updatePageDraft).not.toHaveBeenCalled();
  });

  it("routes an already-active resumed draft once after loading", async () => {
    vi.mocked(getPage).mockResolvedValue(page({ status: "active", version: 4 }));
    const { onPublished } = renderEditor({ draftId: "draft-1" });

    await waitFor(() => expect(onPublished).toHaveBeenCalledTimes(1));
    expect(onPublished).toHaveBeenCalledWith("draft-1", undefined);
  });
});
