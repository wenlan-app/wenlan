import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getChunks, getZoteroDocument, openSearchResult, readZoteroPdf, syncRegisteredSource, type IndexedFileInfo } from "../../../lib/tauri";
import { SourceDocumentReader } from "./SourceDocumentReader";
import ZoteroDocumentReader from "./ZoteroDocumentReader";
import { readZoteroAttachmentSelection, saveZoteroAttachmentSelection } from "./pdfReadingState";

vi.mock("../../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../../lib/tauri")>()),
  getChunks: vi.fn(), getZoteroDocument: vi.fn(), readZoteroPdf: vi.fn(), openSearchResult: vi.fn(), syncRegisteredSource: vi.fn(),
}));
vi.mock("./PdfReader", () => ({ default: ({ data, documentId, page, readingSessionKey, annotations }: { data: Uint8Array; documentId: string; page?: number; readingSessionKey?: string; annotations?: unknown[] }) =>
  <div data-testid="pdf-reader" data-document={documentId} data-page={page ?? "saved"} data-session={readingSessionKey ?? "none"} data-first-byte={data[0]} data-annotations={annotations?.length ?? 0}>PDF</div>,
}));
const file: IndexedFileInfo = { source: "file", source_agent: "zotero", source_id: "zotero::local::PAPER123::paper.md", title: "Paper", chunk_count: 1, last_modified: 0 };
const scroll = vi.fn();
beforeEach(() => {
  vi.resetAllMocks();
  Element.prototype.scrollIntoView = scroll;
  vi.mocked(getChunks).mockResolvedValue([{ id: "chunk", chunk_index: 0, chunk_type: null, language: null, content: [
    "[Annotation](zotero://open-pdf/library/items/SECOND22?page=7&annotation=NOTE1234)",
    "[Unknown attachment](zotero://open-pdf/library/items/FOREIGN1?page=3&annotation=NOTE1234)",
  ].join("\n\n") }]);
  vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], attachments: [
    { key: "FIRST111", title: "First attachment", available: true },
    { key: "SECOND22", title: "Second attachment", available: true },
  ] });
  vi.mocked(readZoteroPdf).mockResolvedValue(new Uint8Array([1, 2]));
  vi.mocked(openSearchResult).mockResolvedValue();
});
function show(readingSessionKey = "page:abc:primary") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SourceDocumentReader file={file} space="Research" readingSessionKey={readingSessionKey} /></QueryClientProvider>);
}
describe("Zotero annotation to embedded PDF", () => {
  it("describes an unspecified citation location without claiming it changed", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const citation = { occurrence: 1, marker: 1, source_kind: "memory" as const, locator: file.source_id, score: 1, status: "verified" as const, scope: "sentence" as const };
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" citation={citation} /></QueryClientProvider>);
    expect(await screen.findByTestId("pdf-reader")).toHaveAttribute("data-page", "saved");
    expect(screen.getByText("This citation does not specify a PDF location.")).toBeInTheDocument();
    expect(screen.queryByText("The cited PDF location is unavailable or has changed.")).not.toBeInTheDocument();
  });
  it("selects only the current document attachment and passes the requested page", async () => {
    show("page:annotation:primary");
    await screen.findByTestId("pdf-reader");
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-session", "page:annotation:primary");
    expect(screen.getByRole("button", { name: "Open in Zotero" })).toHaveClass("source-pdf-open-zotero");
    fireEvent.click(screen.getByText("Source text and notes"));
    fireEvent.click(await screen.findByRole("link", { name: "Annotation" }));
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-document", `${file.source_id}:SECOND22`));
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-page", "7");
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-session", "page:annotation:primary");
    expect(readZoteroPdf).toHaveBeenLastCalledWith(file.source_id, "SECOND22", "Research", undefined);
    expect(scroll).toHaveBeenCalledWith({ block: "start", behavior: "smooth" });
    expect(openSearchResult).not.toHaveBeenCalled();
    const reader = screen.getByTestId("pdf-reader");
    fireEvent.click(screen.getByRole("link", { name: "Annotation" }));
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).not.toBe(reader));
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-page", "7");
  });
  it("ignores a syntactically valid link to an attachment outside this document", async () => {
    show("page:unknown-attachment:primary");
    await screen.findByTestId("pdf-reader");
    fireEvent.click(screen.getByText("Source text and notes"));
    fireEvent.click(await screen.findByRole("link", { name: "Unknown attachment" }));
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-document", `${file.source_id}:FIRST111`);
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-page", "saved");
    expect(readZoteroPdf).toHaveBeenCalledExactlyOnceWith(file.source_id, "FIRST111", "Research", undefined);
    expect(scroll).not.toHaveBeenCalled();
    expect(openSearchResult).not.toHaveBeenCalled();
  });

  it("restores an attachment for the same page session and validates it against the current inventory", async () => {
    const session = "page:attachment-restore:primary";
    const firstView = show(session);
    await screen.findByTestId("pdf-reader");
    fireEvent.change(screen.getByLabelText("PDF attachment"), { target: { value: "SECOND22" } });
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-document", `${file.source_id}:SECOND22`));
    firstView.unmount();

    const restoredView = show(session);
    await screen.findByTestId("pdf-reader");
    await waitFor(() => expect(screen.getByLabelText("PDF attachment")).toHaveValue("SECOND22"));
    expect(readZoteroAttachmentSelection(session, file.source_id, "Research")).toBe("SECOND22");
    restoredView.unmount();

    const staleSession = "page:removed-attachment:primary";
    saveZoteroAttachmentSelection(staleSession, file.source_id, "Research", "REMOVED1");
    show(staleSession);
    await screen.findByTestId("pdf-reader");
    await waitFor(() => expect(screen.getByLabelText("PDF attachment")).toHaveValue("FIRST111"));
    expect(readZoteroAttachmentSelection(staleSession, file.source_id, "Research")).toBe("FIRST111");
  });

  it("keeps a missing attachment openable in Zotero and only offers a safe source page", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], source_url: "https://example.org/paper", attachments: [
      { key: "FIRST111", title: "First", available: false },
    ] });
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" /></QueryClientProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "Open in Zotero" }));
    expect(openSearchResult).toHaveBeenCalledWith("zotero://open-pdf/library/items/FIRST111");
    fireEvent.click(screen.getByRole("button", { name: "Open source page" }));
    expect(openSearchResult).toHaveBeenCalledWith("https://example.org/paper");
    expect(readZoteroPdf).not.toHaveBeenCalled();
  });

  it.each([
    ["https://example.org/paper?id=123#page=3", true],
    ["https://example.org/paper?access_token=secret", false],
    ["https://example.org/paper#access_token=secret", false],
    ["https://example.org/paper?X-Amz-Signature=secret", false],
    ["http://127.0.0.1:7878/api", false],
    ["http://192.168.1.2/paper", false],
    ["http://printer.local/paper", false],
    ["https://user:password@example.org/paper", false],
  ])("offers only safe source URLs: %s", async (sourceUrl, allowed) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], source_url: sourceUrl, attachments: [] });
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" /></QueryClientProvider>);
    await screen.findByText(/No saved PDF is available/);
    const open = screen.queryByRole("button", { name: "Open source page" });
    if (allowed) {
      expect(open).toBeInTheDocument();
      fireEvent.click(open!);
      expect(openSearchResult).toHaveBeenCalledWith(sourceUrl);
    } else expect(open).not.toBeInTheDocument();
  });

  it("retries only the server-verified importing registration and refreshes the document", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const missing = { source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], registered_source_id: "registration-1", attachments: [{ key: "FIRST111", title: "First", available: false }] };
    vi.mocked(getZoteroDocument).mockResolvedValueOnce(missing).mockResolvedValue({ ...missing, attachments: [{ ...missing.attachments[0], available: true }] });
    vi.mocked(syncRegisteredSource).mockResolvedValue({ files_found: 1, ingested: 1, skipped: 0, errors: 0 });
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" /></QueryClientProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "Sync Zotero again" }));
    await screen.findByTestId("pdf-reader");
    expect(syncRegisteredSource).toHaveBeenCalledExactlyOnceWith("registration-1");
    expect(getZoteroDocument).toHaveBeenCalledTimes(2);
  });

  it("retries a pending citation after sync, without repeating a jump on unrelated refresh", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const missing = { source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], registered_source_id: "registration-1", attachments: [{ key: "FIRST111", title: "First", available: false, pdf_digest: "digest" }] };
    const available = { ...missing, attachments: [{ ...missing.attachments[0], available: true }] };
    vi.mocked(getZoteroDocument).mockResolvedValueOnce(missing).mockResolvedValue(available);
    vi.mocked(syncRegisteredSource).mockResolvedValue({ files_found: 1, ingested: 1, skipped: 0, errors: 0 });
    const citation = { occurrence: 1, marker: 1, source_kind: "memory" as const, locator: file.source_id, score: 1, status: "verified" as const, scope: "sentence" as const,
      pdf_target: { attachment_key: "FIRST111", pdf_digest: "digest", page: 4, quote: "quote" } };
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" citation={citation} /></QueryClientProvider>);
    expect(await screen.findByText("The cited PDF location is unavailable or has changed.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Sync Zotero again" }));
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-page", "4"));
    expect(screen.queryByText("The cited PDF location is unavailable or has changed.")).not.toBeInTheDocument();
    const reader = screen.getByTestId("pdf-reader");
    await client.refetchQueries({ queryKey: ["zotero-document", file.source_id, "Research"] });
    expect(screen.getByTestId("pdf-reader")).toBe(reader);
    expect(scroll).toHaveBeenCalledTimes(1);
  });

  it("never pairs new attachment metadata with old cached bytes", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const annotation = { key: "ANNOT001", attachment_key: "FIRST111", page_index: 0, rects: [[1, 2, 3, 4] as [number, number, number, number]], text: "", comment: "", color: "#ff0000" };
    const original = { source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], attachments: [{ key: "FIRST111", title: "First", available: true, pdf_digest: "old" }] };
    const updated = { ...original, annotations: [annotation], attachments: [{ ...original.attachments[0], pdf_digest: "new" }] };
    let finish: (bytes: Uint8Array) => void = () => {};
    vi.mocked(getZoteroDocument).mockResolvedValueOnce(original).mockResolvedValue(updated);
    vi.mocked(readZoteroPdf).mockResolvedValueOnce(new Uint8Array([1])).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" readingSessionKey="replacement" /></QueryClientProvider>);
    expect(await screen.findByTestId("pdf-reader")).toHaveAttribute("data-first-byte", "1");
    await client.refetchQueries({ queryKey: ["zotero-document", file.source_id, "Research"] });
    await waitFor(() => expect(readZoteroPdf).toHaveBeenLastCalledWith(file.source_id, "FIRST111", "Research", "new"));
    expect(screen.queryByTestId("pdf-reader")).not.toBeInTheDocument();
    finish(new Uint8Array([2]));
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-first-byte", "2"));
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-annotations", "1");
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-session", "replacement");
  });

  it("ignores an old in-flight PDF response after the attachment digest changes", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const original = { source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], attachments: [{ key: "FIRST111", title: "First", available: true, pdf_digest: "old" }] };
    vi.mocked(getZoteroDocument).mockResolvedValueOnce(original).mockResolvedValue({ ...original, attachments: [{ ...original.attachments[0], pdf_digest: "new" }] });
    let finishOld: (bytes: Uint8Array) => void = () => {};
    vi.mocked(readZoteroPdf).mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve; })).mockResolvedValueOnce(new Uint8Array([2]));
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" /></QueryClientProvider>);
    await waitFor(() => expect(readZoteroPdf).toHaveBeenCalledWith(file.source_id, "FIRST111", "Research", "old"));
    await client.refetchQueries({ queryKey: ["zotero-document", file.source_id, "Research"] });
    expect(await screen.findByTestId("pdf-reader")).toHaveAttribute("data-first-byte", "2");
    finishOld(new Uint8Array([1]));
    await Promise.resolve();
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-first-byte", "2");
  });

  it("offers scoped sync if cached bytes disappear after metadata was loaded", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const doc = { source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], registered_source_id: "registration-1", attachments: [{ key: "FIRST111", title: "First", available: true, pdf_digest: "digest" }] };
    vi.mocked(getZoteroDocument).mockResolvedValue(doc);
    vi.mocked(readZoteroPdf).mockRejectedValueOnce(new Error("cache missing")).mockResolvedValue(new Uint8Array([2]));
    vi.mocked(syncRegisteredSource).mockResolvedValue({ files_found: 1, ingested: 1, skipped: 0, errors: 0 });
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" /></QueryClientProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "Sync Zotero again" }));
    expect(await screen.findByTestId("pdf-reader")).toHaveAttribute("data-first-byte", "2");
    expect(syncRegisteredSource).toHaveBeenCalledExactlyOnceWith("registration-1");
  });

  it("opens the citation attachment only when its immutable digest matches", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], annotations: [], attachments: [
      { key: "FIRST111", title: "First", available: true, pdf_digest: "a" },
      { key: "SECOND22", title: "Second", available: true, pdf_digest: "b" },
    ] });
    const citation = { occurrence: 1, marker: 1, source_kind: "memory" as const, locator: file.source_id, score: 1, status: "verified" as const, scope: "sentence" as const,
      pdf_target: { attachment_key: "SECOND22", pdf_digest: "b", page: 3, quote: "quoted text" } };
    const view = render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" citation={citation} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-document", `${file.source_id}:SECOND22`));
    expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-page", "3");
    view.rerender(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" citation={{ ...citation, pdf_target: { ...citation.pdf_target, pdf_digest: "stale" } }} /></QueryClientProvider>);
    expect(await screen.findByText("The cited PDF location is unavailable or has changed.")).toBeInTheDocument();
    expect(screen.queryByTestId("pdf-reader")).not.toBeInTheDocument();
  });

  it("requests and labels the exact retained PDF generation without substituting current bytes", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const target = { attachment_key: "FIRST111", pdf_digest: "old-digest", page: 4, quote: "old quote" };
    vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: file.source_id, title: "Old paper", bibtex: "", warnings: [], historical: true, annotations: [], attachments: [{ key: "FIRST111", title: "Old attachment", available: true, pdf_digest: "old-digest" }] } as Awaited<ReturnType<typeof getZoteroDocument>>);
    const citation = { occurrence: 1, marker: 1, source_kind: "memory" as const, locator: file.source_id, score: 1, status: "verified" as const, scope: "sentence" as const, pdf_target: target };
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" citation={citation} /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-page", "4"));
    expect(screen.getByText("Historical PDF version used by this citation.")).toBeInTheDocument();
    expect(getZoteroDocument).toHaveBeenCalledWith(file.source_id, "Research", "FIRST111", "old-digest", true);
    expect(readZoteroPdf).toHaveBeenCalledWith(file.source_id, "FIRST111", "Research", "old-digest", true);
    expect(screen.queryByRole("button", {name:"Open in Zotero"})).not.toBeInTheDocument();
  });

  it("does not load another generation when retained metadata is unavailable", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.mocked(getZoteroDocument).mockRejectedValue(new Error("retained generation unavailable"));
    const citation = { occurrence: 1, marker: 1, source_kind: "memory" as const, locator: file.source_id, score: 1, status: "verified" as const, scope: "sentence" as const, pdf_target: {attachment_key:"FIRST111",pdf_digest:"old-digest",page:4,quote:"old quote"} };
    render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" citation={citation} /></QueryClientProvider>);
    expect(await screen.findByText("This cited PDF version is unavailable.")).toBeInTheDocument();
    expect(readZoteroPdf).not.toHaveBeenCalled();
    expect(screen.queryByTestId("pdf-reader")).not.toBeInTheDocument();
  });

  it("does not carry an attachment selection across source identity changes without remount", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={file.source_id} space="Research" readingSessionKey="same-page" /></QueryClientProvider>);
    await screen.findByTestId("pdf-reader");
    fireEvent.change(screen.getByLabelText("PDF attachment"), { target: { value: "SECOND22" } });
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-document", `${file.source_id}:SECOND22`));

    const nextSource = "zotero::local::PAPER456::paper.md";
    view.rerender(<QueryClientProvider client={client}><ZoteroDocumentReader sourceId={nextSource} space="Research" readingSessionKey="same-page" /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByTestId("pdf-reader")).toHaveAttribute("data-document", `${nextSource}:FIRST111`));
    expect(readZoteroAttachmentSelection("same-page", nextSource, "Research")).toBe("FIRST111");
  });

});
