import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getChunks, getZoteroDocument, openSearchResult, readZoteroPdf, type IndexedFileInfo } from "../../../lib/tauri";
import { SourceDocumentReader } from "./SourceDocumentReader";
import ZoteroDocumentReader from "./ZoteroDocumentReader";
import { readZoteroAttachmentSelection, saveZoteroAttachmentSelection } from "./pdfReadingState";

vi.mock("../../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../../lib/tauri")>()),
  getChunks: vi.fn(), getZoteroDocument: vi.fn(), readZoteroPdf: vi.fn(), openSearchResult: vi.fn(),
}));
vi.mock("./PdfReader", () => ({ default: ({ documentId, page, readingSessionKey }: { documentId: string; page?: number; readingSessionKey?: string }) =>
  <div data-testid="pdf-reader" data-document={documentId} data-page={page ?? "saved"} data-session={readingSessionKey ?? "none"}>PDF</div>,
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
  vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: file.source_id, title: "Paper", bibtex: "", warnings: [], attachments: [
    { key: "FIRST111", title: "First attachment", available: true },
    { key: "SECOND22", title: "Second attachment", available: true },
  ] });
  vi.mocked(readZoteroPdf).mockResolvedValue(new Uint8Array([1, 2]));
});
function show(readingSessionKey = "page:abc:primary") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SourceDocumentReader file={file} space="Research" readingSessionKey={readingSessionKey} /></QueryClientProvider>);
}
describe("Zotero annotation to embedded PDF", () => {
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
    expect(readZoteroPdf).toHaveBeenLastCalledWith(file.source_id, "SECOND22", "Research");
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
    expect(readZoteroPdf).toHaveBeenCalledExactlyOnceWith(file.source_id, "FIRST111", "Research");
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
