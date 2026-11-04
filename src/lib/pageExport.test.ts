import { describe, expect, it } from "vitest";
import { serializePageExport, type PageExportSnapshot } from "./pageExport";

const snapshot: PageExportSnapshot = {
  page: {
    id: "page-1", title: "Research: Notes", content: "# Research: Notes\n\nClaim [1], [2], and [3].\n\n[private](/Users/alice/secret.pdf)\n[file](file:///Users/alice/secret.pdf)",
    version: 7, last_modified: "2026-11-03T12:00:00Z",
    citations: [
      { occurrence: 1, marker: 1, source_kind: "memory", locator: "zotero::P1", score: 1, status: "verified", scope: "sentence" },
      { occurrence: 2, marker: 2, source_kind: "external_file", locator: "/Users/alice/private.pdf", score: 1, status: "verified", scope: "paragraph" },
      { occurrence: 3, marker: 3, source_kind: "external_url", locator: "https://example.org/paper", score: 1, status: "verified", scope: "paragraph" },
    ],
  },
  sources: [{ source: { page_id: "page-1", memory_source_id: "zotero::P1", linked_at: 1 }, memory: {
    source_id: "zotero::P1", title: "A public paper", content: "", summary: null, memory_type: "research", domain: null,
    source_agent: "zotero", confidence: 1, confirmed: true, pinned: false, supersedes: null, last_modified: 1, chunk_count: 1,
  } }],
  zoteroDocuments: [{ sourceId: "zotero::P1", title: "A public paper", bibtex: "@article{paper2024, title={A paper}, url={https://example.org/paper}}" }],
};

describe("page snapshot serialization", () => {
  it("serializes Markdown as the saved version with linked citation footnotes and no local targets", () => {
    const output = serializePageExport(snapshot, "markdown");
    expect(output).toContain("version 7");
    expect(output).toContain("Claim [^wenlan-source-1], [^wenlan-source-2], and [^wenlan-source-3].");
    expect(output).toContain("[^wenlan-source-1]: A public paper — https://example.org/paper");
    expect(output).toContain("Local file reference (path omitted)");
    expect(output).not.toContain("/Users/alice");
    expect(output).not.toContain("file://");
  });

  it("renders safe semantic HTML with citation links and a source list", () => {
    const output = serializePageExport(snapshot, "html");
    expect(output).toContain("<h1>Research: Notes</h1>");
    expect(output).toContain('href="#citation-1"');
    expect(output).toContain('id="citation-1"');
    expect(output).toContain('href="https://example.org/paper"');
    expect(output).not.toContain("/Users/alice");
    expect(output).not.toContain("file://");
    expect(output).not.toContain("<script");
  });

  it("exports indexed Zotero file and webpage citations by their saved bibliography identity", () => {
    const sourceId = "zotero::server/library/item/space";
    const zoteroSnapshot: PageExportSnapshot = {
      ...snapshot,
      page: {
        ...snapshot.page,
        content: "Claim [1], [2], and [3].",
        citations: [
          { occurrence: 1, marker: 1, source_kind: "memory", locator: sourceId, score: 1, status: "verified", scope: "sentence" },
          { occurrence: 2, marker: 2, source_kind: "external_file", locator: sourceId, score: 1, status: "verified", scope: "sentence" },
          { occurrence: 3, marker: 3, source_kind: "external_url", locator: sourceId, score: 1, status: "verified", scope: "sentence" },
        ],
      },
      sources: [{ source: { page_id: "page-1", memory_source_id: sourceId, linked_at: 1 }, memory: {
        ...snapshot.sources[0].memory!, source_id: sourceId, title: "A public paper", source_agent: "zotero",
      } }],
      zoteroDocuments: [{ sourceId, title: "A public paper", bibtex: "@article{paper2024, title={A paper}, url={https://example.org/paper}}" }],
    };

    const markdown = serializePageExport(zoteroSnapshot, "markdown");
    expect(markdown).toContain("Claim [^wenlan-source-1], [^wenlan-source-1], and [^wenlan-source-1].");
    expect(markdown.match(/\[\^wenlan-source-1\]: A public paper/g)).toHaveLength(1);
    expect(markdown).not.toContain("Local file reference");
    expect(markdown).not.toContain(sourceId);

    const html = serializePageExport(zoteroSnapshot, "html");
    expect(html.match(/<li id="citation-1">/g)).toHaveLength(1);
    expect(html).toContain('href="#citation-1"');
    expect(html).not.toContain("Local file reference");
    expect(html).not.toContain(sourceId);
  });

  it("exports only nonempty bibliography entries and refuses empty BibTeX", () => {
    const output = serializePageExport({ ...snapshot, zoteroDocuments: [{ ...snapshot.zoteroDocuments[0], bibtex: "@article{paper2024, title={A paper},\n  file = {/Users/alice/library/paper.pdf},\n  url = {https://example.org/paper}\n}" }] }, "bibtex");
    expect(output).toContain("title={A paper}");
    expect(output).toContain("url = {https://example.org/paper}");
    expect(output).not.toContain("/Users/alice");
    expect(() => serializePageExport({ ...snapshot, zoteroDocuments: [] }, "bibtex")).toThrow(/no Zotero bibliography/i);
  });

  it("removes unsafe URL targets while retaining ordinary text and safe schemes", () => {
    const output = serializePageExport({ ...snapshot, page: { ...snapshot.page, citations: [], content: "[web](https://example.org/x) [mail](mailto:a@example.org) [jump](#part) [bad](javascript:alert(1)) [relative](docs/a.md)" } }, "markdown");
    expect(output).toContain("[web](https://example.org/x)");
    expect(output).toContain("[mail](mailto:a@example.org)");
    expect(output).toContain("[jump](#part)");
    expect(output).not.toContain("javascript:");
    expect(output).not.toContain("docs/a.md");
  });
});
