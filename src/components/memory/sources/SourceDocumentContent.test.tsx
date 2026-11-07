import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import SourceDocumentContent from "./SourceDocumentContent";

const href = "zotero://open-pdf/library/items/AB12CD34?page=7&annotation=ZX98YU76";
describe("source Markdown Zotero links", () => {
  it("sends only a whitelisted annotation target to the embedded reader callback", () => {
    const open = vi.fn();
    render(<SourceDocumentContent title="Paper" content={`[Annotation](${href})`} onZoteroAnnotation={open} />);
    const link = screen.getByRole("link", { name: "Annotation" });
    expect(link).toHaveAttribute("href", "#zotero-pdf");
    expect(link).not.toHaveAttribute("target");
    fireEvent.click(link);
    expect(open).toHaveBeenCalledExactlyOnceWith({ attachmentKey: "AB12CD34", page: 7, annotationKey: "ZX98YU76" });
  });
  it("retains the default URL sanitizer for all other links", () => {
    const open = vi.fn();
    render(<SourceDocumentContent title="Paper" onZoteroAnnotation={open} content={[
      "[Bad](javascript:run)", "[Wrong action](zotero://select/library/items/AB12CD34)",
      `[Extra action](${href}&execute=1)`, "[Website](https://example.com/paper)",
    ].join("\n\n")} />);
    for (const text of ["Bad", "Wrong action", "Extra action"]) {
      const link = screen.getByText(text).closest("a")!;
      expect(link.getAttribute("href") ?? "").toBe("");
      fireEvent.click(link);
    }
    expect(open).not.toHaveBeenCalled();
    expect(screen.getByRole("link", { name: "Website" })).toHaveAttribute("href", "https://example.com/paper");
  });
  it("does not enable Zotero URLs outside a reader that provides the scoped callback", () => {
    render(<SourceDocumentContent title="Paper" content={`[Annotation](${href})`} />);
    expect(screen.getByText("Annotation").closest("a")?.getAttribute("href") ?? "").toBe("");
  });
});
