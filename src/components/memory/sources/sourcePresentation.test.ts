// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { getSourceAppFromUrl, getSourceKind, getSourcePresentation, sourceLocatorLabel, type SourcePresentationInput } from "./sourcePresentation";

const labels = { document: "Document", webExcerpt: "Web excerpt" };
const source = (patch: Partial<SourcePresentationInput> = {}): SourcePresentationInput => ({
  source: "file",
  source_id: "/Users/me/Research/notes.pdf",
  title: "Research notes",
  url: null,
  ...patch,
});

describe("source presentation", () => {
  it("labels a stored source id by its file name, never the internal id", () => {
    expect(sourceLocatorLabel("obsidian-vault::/Users/me/vault/Books/Reading.md::2")).toBe("Reading.md");
    expect(sourceLocatorLabel("obsidian-vault::C:\\Users\\me\\vault\\Reading.md::0")).toBe("Reading.md");
    expect(sourceLocatorLabel("folder::/Users/me/Research/")).toBe("Research");
    expect(sourceLocatorLabel("obsidian-vault::/Users/me/vault/Note.md::0")).not.toContain("obsidian-vault");
  });

  it("leaves ids without a namespace as they are", () => {
    expect(sourceLocatorLabel("mem_abc123")).toBe("mem_abc123");
    expect(sourceLocatorLabel("https://example.com/a")).toBe("https://example.com/a");
  });

  it("classifies indexed Obsidian sections by the underlying file without changing their identity", () => {
    const file = source({ source: "memory", source_agent: "obsidian", source_id: "vault::/notes/Books/Reading.md::2" });
    expect(getSourcePresentation(file, labels).metadata).toBe("Markdown · Books");
    expect(getSourceKind(file)).toBe("text");
    expect(file.source_id).toBe("vault::/notes/Books/Reading.md::2");
    expect(getSourcePresentation(source({ source: "obsidian", source_id: "vault::C:\\Notes\\Reading.md::0" }), labels).format).toBe("Markdown");
  });

  it("keeps a literal ordinal suffix in non-Obsidian filenames", () => {
    expect(getSourceKind(source({ source: "directory", source_id: "folder::/notes/Reading.md::2" }))).toBe("file");
  });
  it("keeps a remote PDF classified by format and uses its host as provenance", () => {
    const file = source({
      source: "webpage",
      source_id: "https://docs.example.test/Guide.PDF?download=1#page=2",
      url: null,
    });
    expect(getSourceKind(file)).toBe("pdf");
    expect(getSourcePresentation(file, labels)).toEqual({
      kind: "pdf", app: null, iconKind: "pdf", format: "PDF", origin: "docs.example.test", metadata: "PDF · docs.example.test",
    });
  });

  it("uses a local Windows path for format and parent-folder metadata, ignoring a misleading URL", () => {
    const file = source({
      source: "directory",
      source_id: "folder-1::C:\\Users\\me\\Research\\Notes.MD",
      title: "Meeting with Alice.pdf",
      url: "https://unrelated.example.test/article",
    });
    expect(getSourceKind(file)).toBe("text");
    expect(getSourcePresentation(file, labels).metadata).toBe("Markdown · Research");
  });

  it("recognizes image and plain-text formats from URL paths with query strings and fragments", () => {
    const image = source({ source: "webpage", source_id: "https://cdn.example.test/Photo.JPEG?v=2#crop" });
    const text = source({ source: "file", source_id: "https://cdn.example.test/readme.TXT?raw=true" });
    expect(getSourceKind(image)).toBe("image");
    expect(getSourcePresentation(image, labels).metadata).toBe("JPEG · cdn.example.test");
    expect(getSourceKind(text)).toBe("text");
    expect(getSourcePresentation(text, labels).metadata).toBe("TXT · cdn.example.test");
  });

  it("uses the webpage label and host for URLs without a known file format", () => {
    const file = source({ source: "webpage", source_id: "https://news.example.test/story?ref=notes#top" });
    expect(getSourceKind(file)).toBe("webpage");
    expect(getSourcePresentation(file, labels).metadata).toBe("Web excerpt · news.example.test");
  });

  it("recognizes webpage route suffixes while keeping unknown remote files generic", () => {
    const webpage = source({ source: "webpage", source_id: "https://example.test/article.html?x=1#top" });
    const archive = source({ source: "file", source_id: "https://cdn.example.test/archive.zip?download=1" });
    const csv = source({ source: "file", source_id: "https://cdn.example.test/export.csv" });
    expect(getSourceKind(webpage)).toBe("webpage");
    expect(getSourcePresentation(webpage, labels).metadata).toBe("Web excerpt · example.test");
    expect(getSourceKind(archive)).toBe("file");
    expect(getSourceKind(csv)).toBe("file");
  });

  it("keeps unknown real paths generic even when their prose title ends in a known extension", () => {
    const file = source({ source_id: "import::/archive/meeting.backup", title: "Project notes.pdf" });
    expect(getSourceKind(file)).toBe("file");
    expect(getSourcePresentation(file, labels)).toEqual({
      kind: "file", app: null, iconKind: "file", format: "Document", origin: "archive", metadata: "Document · archive",
    });
  });

  it("uses a title extension only when no real path or URL is available", () => {
    const file = source({ source_id: "opaque-source-id", title: "Notes.md", url: null });
    expect(getSourceKind(file)).toBe("text");
    expect(getSourcePresentation(file, labels).metadata).toBe("Markdown");
  });

  describe("saved documents from other apps", () => {
    it.each([
      ["https://mail.google.com/mail/u/0/#inbox/FMfcgzQ", "gmail"],
      ["https://docs.google.com/document/d/abc/edit", "googleDrive"],
      ["https://drive.google.com/file/d/abc/view", "googleDrive"],
      ["https://notion.so/Q4-roadmap-1a2b3c", "notion"],
      ["https://www.notion.so/team/Q4-roadmap-1a2b3c", "notion"],
      ["https://team.notion.site/Public-page-1a2b3c", "notion"],
      ["https://app.notion.com/p/Q4-roadmap-1a2b3c", "notion"],
      ["https://slack.com/archives/C024/p1", "slack"],
      ["https://acme.slack.com/archives/C024/p1712345678", "slack"],
      ["https://github.com/wenlan-app/wenlan/issues/12", "github"],
      ["HTTPS://Mail.Google.COM/mail/u/0/", "gmail"],
    ])("recognizes %s as %s", (url, app) => {
      expect(getSourceAppFromUrl(url)).toBe(app);
    });

    it.each([
      "https://example.com/blog/local-first",
      "https://google.com/search?q=gmail",
      "https://evilmail.google.com.example.com/",
      "https://notgithub.com/x",
      "https://notion.so.example.com/x",
      "https://fakeslack.com/x",
      "https://gist.github.com/x",
      "ftp://mail.google.com/x",
      "mail.google.com",
      "",
      null,
      undefined,
    ])("treats %s as an ordinary web page", (url) => {
      expect(getSourceAppFromUrl(url)).toBeNull();
    });

    it("names the app and drops the generic format and raw host for a saved Gmail message", () => {
      const file = source({ source: "webpage", source_id: "https://mail.google.com/mail/u/0/#inbox/FMfcgzQ", title: "Invoice" });
      expect(getSourceKind(file)).toBe("webpage");
      expect(getSourcePresentation(file, labels)).toEqual({
        kind: "webpage", app: "gmail", iconKind: "gmail", format: "Gmail", origin: null, metadata: "Gmail",
      });
    });

    it("keeps the app name identical whatever the UI labels say", () => {
      const file = source({ source: "webpage", source_id: "https://www.notion.so/team/Q4-roadmap-1a2b3c" });
      expect(getSourcePresentation(file, { document: "文件", webExcerpt: "網頁" }).metadata).toBe("Notion");
    });

    it("leaves an unrecognized web page exactly as before", () => {
      const file = source({ source: "webpage", source_id: "https://example.com/blog/local-first" });
      expect(getSourcePresentation(file, labels)).toEqual({
        kind: "webpage", app: null, iconKind: "webpage", format: "Web excerpt", origin: "example.com", metadata: "Web excerpt · example.com",
      });
    });

    it("keeps file formats ahead of the app when the link points at a file", () => {
      const file = source({ source: "webpage", source_id: "https://github.com/acme/repo/raw/main/spec.pdf" });
      expect(getSourcePresentation(file, labels)).toMatchObject({ kind: "pdf", app: null, iconKind: "pdf", metadata: "PDF · github.com" });
    });

    it("does not treat a local file with an attached app URL as that app", () => {
      const file = source({ source: "file", source_id: "/Users/me/export.md", url: "https://mail.google.com/mail/u/0/" });
      expect(getSourcePresentation(file, labels).app).toBeNull();
    });
  });
});
