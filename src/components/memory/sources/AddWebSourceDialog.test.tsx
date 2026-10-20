// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { StrictMode, useState } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { toast } from "sonner";
import { ingestWebpage, listIndexedFiles, type IndexedFileInfo } from "../../../lib/tauri";
import AddWebSourceDialog from "./AddWebSourceDialog";

vi.mock("../../../lib/tauri", () => ({ ingestWebpage: vi.fn(), listIndexedFiles: vi.fn() }));
vi.mock("sonner", () => ({ toast: vi.fn() }));

function renderDialog() {
  const onClose = vi.fn();
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const view = render(<QueryClientProvider client={client}><AddWebSourceDialog onClose={onClose} /></QueryClientProvider>);
  return { onClose, invalidate, ...view };
}

function fillExcerpt(url = "https://example.com/article") {
  fireEvent.change(screen.getByLabelText("Web address"), { target: { value: url } });
  fireEvent.change(screen.getByLabelText("Title (optional)"), { target: { value: "Reference article" } });
  fireEvent.change(screen.getByLabelText("Text from the web page"), { target: { value: "  First paragraph.\n\nSecond paragraph.  " } });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}

function renderReopeningDialog() {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const closed = vi.fn();
  function Host() {
    const [open, setOpen] = useState(true);
    return <>
      <button onClick={() => setOpen(true)}>Reopen excerpt dialog</button>
      {open && <AddWebSourceDialog onClose={() => { closed(); setOpen(false); }} />}
    </>;
  }
  const view = render(<StrictMode><QueryClientProvider client={client}><Host /></QueryClientProvider></StrictMode>);
  return { closed, invalidate, ...view };
}

const dismissPaths = ["Cancel", "Close", "Escape", "backdrop"] as const;
function dismiss(path: typeof dismissPaths[number]) {
  if (path === "Escape") fireEvent.keyDown(document, { key: "Escape" });
  else if (path === "backdrop") fireEvent.click(screen.getByRole("dialog").parentElement!);
  else fireEvent.click(screen.getByRole("button", { name: path }));
}
function openSecondDraft() {
  fireEvent.click(screen.getByRole("button", { name: "Reopen excerpt dialog" }));
  fillExcerpt("https://second.example/new-draft");
  fireEvent.change(screen.getByLabelText("Title (optional)"), { target: { value: "Second untouched title" } });
  fireEvent.change(screen.getByLabelText("Text from the web page"), { target: { value: "Second dialog unsaved text.\n  Preserve spacing.  " } });
}
function expectSecondDraft() {
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(screen.getByLabelText("Web address")).toHaveValue("https://second.example/new-draft");
  expect(screen.getByLabelText("Title (optional)")).toHaveValue("Second untouched title");
  expect(screen.getByLabelText("Text from the web page")).toHaveValue("Second dialog unsaved text.\n  Preserve spacing.  ");
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(listIndexedFiles).mockResolvedValue([]);
  vi.mocked(ingestWebpage).mockResolvedValue({ chunks_created: 1, document_id: "web-document" });
});

describe("AddWebSourceDialog", () => {
  it("sends the supplied URL, title and unchanged excerpt only after Save", async () => {
    const { onClose, invalidate } = renderDialog();
    fillExcerpt();
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(listIndexedFiles).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText(/it does not fetch the website/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(ingestWebpage).toHaveBeenCalledOnce());
    expect(vi.mocked(ingestWebpage).mock.calls[0][0]).toEqual({
      url: "https://example.com/article", title: "Reference article", content: "  First paragraph.\n\nSecond paragraph.  ", create_only: true,
    });
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["indexedFiles"] });
    expect(toast).toHaveBeenCalledWith("Web excerpt saved");
  });

  it.each(["ftp://example.com/article", "javascript:alert(1)", "https://user:password@example.com/article", "not-an-address"])(
    "rejects unsafe or incomplete address %s before ingestion", async (url) => {
      const { onClose } = renderDialog();
      fillExcerpt(url);
      // Exercise the application's own guard independent of native form validation.
      fireEvent.submit(screen.getByRole("button", { name: "Save excerpt" }).closest("form")!);
      expect(await screen.findByRole("alert")).toHaveTextContent("Enter a complete http or https address.");
      expect(ingestWebpage).not.toHaveBeenCalled();
      expect(onClose).not.toHaveBeenCalled();
    },
  );

  it("requires meaningful text and uses the hostname when the optional title is blank", async () => {
    renderDialog();
    fillExcerpt();
    fireEvent.change(screen.getByLabelText("Text from the web page"), { target: { value: " \n " } });
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Paste the text you want to keep.");
    expect(ingestWebpage).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Text from the web page"), { target: { value: "Saved excerpt." } });
    fireEvent.change(screen.getByLabelText("Title (optional)"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(ingestWebpage).toHaveBeenCalledOnce());
    expect(vi.mocked(ingestWebpage).mock.calls[0][0]).toEqual({
      url: "https://example.com/article", title: "example.com", content: "Saved excerpt.", create_only: true,
    });
  });

  it("retains all input after an error and allows an explicit retry without a success callback", async () => {
    vi.mocked(ingestWebpage).mockRejectedValueOnce(new Error("Daemon unavailable"));
    const { onClose, invalidate } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not add the source");
    expect(screen.getByRole("alert")).toHaveTextContent("Daemon unavailable");
    expect(screen.getByLabelText("Web address")).toHaveValue("https://example.com/article");
    expect(screen.getByLabelText("Title (optional)")).toHaveValue("Reference article");
    expect(screen.getByLabelText("Text from the web page")).toHaveValue("  First paragraph.\n\nSecond paragraph.  ");
    expect(onClose).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ingestWebpage).toHaveBeenCalledTimes(2);
  });

  it("does not overwrite a duplicate webpage URL until explicit replacement", async () => {
    const existing: IndexedFileInfo = {
      source: "webpage", source_id: "https://example.com/article", title: "Earlier excerpt", chunk_count: 1, last_modified: 1,
    };
    vi.mocked(listIndexedFiles).mockResolvedValue([existing]);
    const { onClose, invalidate } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Your new text has not been saved yet.");
    const replace = screen.getByRole("button", { name: "Replace existing excerpt" });
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    expect(listIndexedFiles).toHaveBeenCalledOnce();
    fireEvent.click(replace);
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ingestWebpage).toHaveBeenCalledExactlyOnceWith({
      url: "https://example.com/article", title: "Reference article", content: "  First paragraph.\n\nSecond paragraph.  ", create_only: false,
    });
    expect(listIndexedFiles).toHaveBeenCalledOnce();
  });

  it.each(["WEBPAGE_ALREADY_EXISTS", new Error("WEBPAGE_ALREADY_EXISTS")])("requires consent after a concurrent creator wins: %s", async error => {
    vi.mocked(ingestWebpage).mockRejectedValueOnce(error);
    const { onClose, invalidate } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Your new text has not been saved yet.");
    expect(screen.getByLabelText("Web address")).toHaveValue("https://example.com/article");
    expect(screen.getByLabelText("Title (optional)")).toHaveValue("Reference article");
    expect(screen.getByLabelText("Text from the web page")).toHaveValue("  First paragraph.\n\nSecond paragraph.  ");
    expect(onClose).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    expect(ingestWebpage).toHaveBeenCalledTimes(1);
    expect(vi.mocked(ingestWebpage).mock.calls[0][0].create_only).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Replace existing excerpt" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ingestWebpage).toHaveBeenCalledTimes(2);
    expect(vi.mocked(ingestWebpage).mock.calls[1][0]).toEqual({
      url: "https://example.com/article", title: "Reference article", content: "  First paragraph.\n\nSecond paragraph.  ", create_only: false,
    });
  });

  it("retains the draft when an older daemon lacks create-only without trying replacement", async () => {
    vi.mocked(ingestWebpage).mockRejectedValueOnce(new Error("HTTP 404 Not Found"));
    const { onClose } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("HTTP 404 Not Found");
    expect(screen.getByLabelText("Text from the web page")).toHaveValue("  First paragraph.\n\nSecond paragraph.  ");
    expect(screen.queryByRole("button", { name: "Replace existing excerpt" })).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    expect(ingestWebpage).toHaveBeenCalledTimes(1);
    expect(vi.mocked(ingestWebpage).mock.calls[0][0].create_only).toBe(true);
  });

  it.each([
    ["Web address", "https://example.com/another"],
    ["Title (optional)", "Changed title"],
    ["Text from the web page", "Changed draft text."],
  ])("cancels replacement confirmation when the draft field %s changes", async (label, value) => {
    const existing = (url: string): IndexedFileInfo => ({ source: "webpage", source_id: url, title: "Earlier excerpt", chunk_count: 1, last_modified: 1 });
    vi.mocked(listIndexedFiles).mockResolvedValue([existing("https://example.com/article"), existing("https://example.com/another")]);
    const { onClose } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await screen.findByRole("button", { name: "Replace existing excerpt" });
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
    expect(screen.queryByRole("button", { name: "Replace existing excerpt" })).not.toBeInTheDocument();
    expect(screen.queryByText(/Your new text has not been saved yet/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await screen.findByRole("button", { name: "Replace existing excerpt" });
    expect(listIndexedFiles).toHaveBeenCalledTimes(2);
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("fails closed when the current inventory cannot be read", async () => {
    vi.mocked(listIndexedFiles).mockRejectedValue(new Error("Inventory unavailable"));
    const { onClose, invalidate } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Inventory unavailable");
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Text from the web page")).toHaveValue("  First paragraph.\n\nSecond paragraph.  ");
    expect(screen.queryByRole("button", { name: "Replace existing excerpt" })).not.toBeInTheDocument();
  });

  it("contains keyboard focus, dismisses on Escape and restores the trigger", () => {
    render(<button>Open source dialog</button>);
    const trigger = screen.getByRole("button", { name: "Open source dialog" });
    trigger.focus();
    const { onClose, unmount } = renderDialog();
    const close = screen.getByRole("button", { name: "Close" });
    const save = screen.getByRole("button", { name: "Save excerpt" });
    expect(close).toHaveFocus();
    fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
    expect(save).toHaveFocus();
    fireEvent.keyDown(document, { key: "Tab" });
    expect(close).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape", isComposing: true });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
    unmount();
    expect(trigger).toHaveFocus();
  });
  it.each(dismissPaths)("%s during inventory cancels the preflight and preserves reopened dialog B", async path => {
    const inventory = deferred<IndexedFileInfo[]>();
    vi.mocked(listIndexedFiles).mockReturnValueOnce(inventory.promise);
    const { closed, invalidate } = renderReopeningDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(listIndexedFiles).toHaveBeenCalledOnce());
    dismiss(path);
    expect(closed).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    openSecondDraft();
    await act(async () => { inventory.resolve([]); });
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    expect(closed).toHaveBeenCalledOnce();
    expectSecondDraft();
  });

  it.each(dismissPaths)("%s after ingest starts still refreshes inventory without affecting reopened dialog B", async path => {
    const ingest = deferred<Awaited<ReturnType<typeof ingestWebpage>>>();
    vi.mocked(ingestWebpage).mockReturnValueOnce(ingest.promise);
    const { closed, invalidate } = renderReopeningDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(ingestWebpage).toHaveBeenCalledOnce());
    dismiss(path);
    expect(closed).toHaveBeenCalledOnce();
    openSecondDraft();
    await act(async () => { ingest.resolve({ chunks_created: 1, document_id: "old-dialog-write" }); });
    await waitFor(() => expect(invalidate).toHaveBeenCalledExactlyOnceWith({ queryKey: ["indexedFiles"] }));
    expect(ingestWebpage).toHaveBeenCalledOnce();
    expect(toast).not.toHaveBeenCalled();
    expect(closed).toHaveBeenCalledOnce();
    expectSecondDraft();
  });

  it("invalidates preflight synchronously on cancel even before parent unmount", async () => {
    const inventory = deferred<IndexedFileInfo[]>();
    vi.mocked(listIndexedFiles).mockReturnValueOnce(inventory.promise);
    // This callback intentionally leaves the canceled instance mounted.
    const { onClose, invalidate } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(listIndexedFiles).toHaveBeenCalledOnce());
    dismiss("Cancel");
    await act(async () => { inventory.resolve([]); });
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("invalidates preflight on unmount without an explicit cancel action", async () => {
    const inventory = deferred<IndexedFileInfo[]>();
    vi.mocked(listIndexedFiles).mockReturnValueOnce(inventory.promise);
    const { onClose, invalidate, unmount } = renderDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(listIndexedFiles).toHaveBeenCalledOnce());
    unmount();
    await act(async () => { inventory.resolve([]); });
    expect(ingestWebpage).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("lets an active StrictMode dialog finish its own save", async () => {
    const { closed, invalidate } = renderReopeningDialog();
    fillExcerpt();
    fireEvent.click(screen.getByRole("button", { name: "Save excerpt" }));
    await waitFor(() => expect(closed).toHaveBeenCalledOnce());
    expect(ingestWebpage).toHaveBeenCalledOnce();
    expect(invalidate).toHaveBeenCalledExactlyOnceWith({ queryKey: ["indexedFiles"] });
    expect(toast).toHaveBeenCalledWith("Web excerpt saved");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

});
