// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { SourceClassificationReview } from "./SourceClassificationReview";

const changes = [{ source: "file", source_id: "zotero::A", title: "Paper A", source_content_digest: "digest", expected_decision_revision: 2, before_tags: ["human"], add_tags: ["methods"], remove_tags: ["old"], reason: "Uses interviews" }];
describe("classification review", () => {
  it("shows the current tags and proposed changes with their reason", () => {
    render(<SourceClassificationReview changes={changes} editing={false} onChange={vi.fn()} />);
    expect(screen.getByText("Paper A")).toBeInTheDocument();
    expect(screen.getByText(/Current tags: human/)).toBeInTheDocument();
    expect(screen.getByText(/Add tags: methods/)).toBeInTheDocument();
    expect(screen.getByText(/Remove tags: old/)).toBeInTheDocument();
    expect(screen.getByText(/Uses interviews/)).toBeInTheDocument();
  });
  it("edits only the requested proposal while preserving the CAS fields", () => {
    const onChange = vi.fn();
    render(<SourceClassificationReview changes={changes} editing onChange={onChange} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Paper A Add tags" }), { target: { value: "fieldwork, methods" } });
    expect(onChange).toHaveBeenCalledWith([{ ...changes[0], add_tags: ["fieldwork", "methods"] }]);
  });
});
