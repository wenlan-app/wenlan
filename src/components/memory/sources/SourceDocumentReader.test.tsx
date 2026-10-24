// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { getChunks, openSearchResult, type IndexedFileInfo } from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import { SourceDocumentLayout, SourceDocumentPage, SourceDocumentReader } from "./SourceDocumentReader";

vi.mock("../../../lib/tauri", () => ({ getChunks: vi.fn(), openSearchResult: vi.fn() }));
vi.mock("./SourceAnnotations", () => ({
  default: ({ renderRelatedPages }: { renderRelatedPages?: (content: ReactNode) => ReactNode }) => <>
    <section aria-label="Source description">My description.</section>
    {renderRelatedPages?.(<section><h3>Pages</h3><button>Related page</button></section>)}
  </>,
}));
const file: IndexedFileInfo = { source: "webpage", source_id: "https://example.com/article", title: "Article", chunk_count: 1, last_modified: 1791680000 };
function wrap(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getChunks).mockResolvedValue([]);
  vi.mocked(openSearchResult).mockResolvedValue(undefined);
  vi.stubGlobal("matchMedia", vi.fn((query: string) => ({ matches: query.includes("1100"), media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
});

describe("simple link source", () => {
  it("shows the address and editable description without extracting or fetching webpage text", () => {
    wrap(<SourceDocumentReader file={file} space="Reading" />);
    expect(screen.getByText("My description.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open original" })).toHaveAttribute("href", file.source_id);
    expect(screen.queryByRole("button", { name: /Refresh|Open original/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(getChunks).not.toHaveBeenCalled();
    expect(openSearchResult).not.toHaveBeenCalled();
  });
  it("hands a link to the system browser only when clicked", () => {
    wrap(<SourceDocumentReader file={file} />);
    fireEvent.click(screen.getByRole("link", { name: "Open original" }));
    expect(openSearchResult).toHaveBeenCalledExactlyOnceWith(file.source_id);
  });
  it("defaults to the common Info sidebar and moves related Pages out of the reading body", () => {
    const { container } = wrap(<SourceDocumentPage file={file} space="Reading" />);
    const reader = container.querySelector(".reading-document-frame")!;
    expect(reader.querySelector("[data-detail-reading]")).toBeNull();
    expect(reader).toHaveTextContent("My description.");
    expect(reader).not.toHaveTextContent("Related page");
    const info = screen.getByRole("complementary", { name: "Info" });
    expect(info).toHaveClass("page-info-drawer--inspector");
    expect(info).toHaveTextContent("Related page");
    expect(info).toHaveTextContent("Reading");
    expect(screen.getByRole("button", { name: "Info" })).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
    expect(screen.getByText("My description.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByRole("complementary")).toHaveTextContent("Related page");
  });
  it.each([
    ["en", "Space"],
    ["zh-Hans", "空间"],
    ["zh-Hant", "空間"],
  ])("labels the Space in the Info panel in %s", async (lng, label) => {
    await i18n.changeLanguage(lng);
    try {
      wrap(<SourceDocumentPage file={file} space="Reading" />);
      const info = screen.getByRole("complementary");
      const term = Array.from(info.querySelectorAll("dt")).find((dt) => dt.nextElementSibling?.textContent === "Reading");
      expect(term?.textContent).toBe(label);
    } finally {
      await i18n.changeLanguage("en");
    }
  });
  it("starts with Info collapsed on a narrow window", () => {
    vi.stubGlobal("matchMedia", vi.fn((query: string) => ({ matches: false, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
    wrap(<SourceDocumentPage file={file} />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Info" })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByRole("dialog", { name: "Info" })).toHaveTextContent("Related page");
  });
});

describe("full local source layout", () => {
  it("uses the Space carried by a source destination when fetching the full document", async () => {
    vi.mocked(getChunks).mockResolvedValue([{ id: "scoped", content: "Scoped document text.", chunk_index: 0, chunk_type: null, language: null }]);
    wrap(<SourceDocumentPage file={{ ...file, source: "file", source_id: "shared.md", space: "Reading" }} />);
    await screen.findByText("Scoped document text.");
    expect(getChunks).toHaveBeenCalledExactlyOnceWith("file", "shared.md", "Reading");
  });

  it("shares the long-form reading and Info slots without fetching indexed chunks", () => {
    const localFile: IndexedFileInfo = { source: "memory", source_agent: "folder", source_id: "folder::/notes/reading.md", title: "reading.md", chunk_count: 0, last_modified: 0 };
    const { container } = wrap(<SourceDocumentLayout file={localFile} content={<p>Complete local Markdown.</p>} infoContent={<p>File metadata.</p>} />);
    const reading = container.querySelector("[data-detail-reading=long-form]")!;
    expect(reading).toHaveTextContent("Complete local Markdown.");
    expect(reading).not.toHaveTextContent("File metadata.");
    expect(screen.getByRole("complementary", { name: "Info" })).toHaveTextContent("File metadata.");
    expect(getChunks).not.toHaveBeenCalled();
  });
});
