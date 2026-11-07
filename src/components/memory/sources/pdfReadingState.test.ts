import { describe, expect, it } from "vitest";
import { readPdfReadingState, savePdfReadingState } from "./pdfReadingState";

describe("PDF reading state", () => {
  it("restores a page, zoom and search only for the same page session and attachment", () => {
    const session = "state-test-paper-context";
    const state = { page: 8, scale: "1.25", query: "retrieval" };
    savePdfReadingState(session, "zotero-paper:attachment-a", state);

    expect(readPdfReadingState(session, "zotero-paper:attachment-a")).toEqual(state);
    expect(readPdfReadingState(session, "zotero-paper:attachment-b")).toBeUndefined();
    expect(readPdfReadingState(`${session}:another-wiki`, "zotero-paper:attachment-a")).toBeUndefined();
  });

  it("rejects invalid page values", () => {
    const session = "invalid-state-test-paper-context";
    savePdfReadingState(session, "paper:attachment", { page: 0, scale: "page-width", query: "" });
    expect(readPdfReadingState(session, "paper:attachment")).toBeUndefined();
  });

  it("evicts old entries to keep the session cache bounded", () => {
    const session = "bounded-state-test";
    for (let index = 0; index < 101; index++) {
      savePdfReadingState(session, `paper-${index}:attachment`, { page: 1, scale: "page-width", query: "" });
    }
    expect(readPdfReadingState(session, "paper-0:attachment")).toBeUndefined();
    expect(readPdfReadingState(session, "paper-100:attachment")).toEqual({ page: 1, scale: "page-width", query: "" });
  });
});
