// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import RelatedPages from "./RelatedPages";
import PageInfo from "./PageInfo";

describe("RelatedPages", () => {
  it("renders nothing when there are no outbound links", () => {
    const { container } = render(<RelatedPages outbound={[]} onPageClick={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders resolved links as clickable cards", async () => {
    const user = userEvent.setup();
    const onPageClick = vi.fn();
    render(
      <RelatedPages
        outbound={[{ label: "Resolved Link", target_page_id: "page-2", target_title: "New title" }]}
        onPageClick={onPageClick}
      />,
    );
    const section = screen.getByLabelText("Linked pages");
    await user.click(within(section).getByRole("button", { name: "New title" }));
    expect(onPageClick).toHaveBeenCalledWith("page-2");
  });

  it("renders unresolved links muted and inert", () => {
    render(
      <RelatedPages
        outbound={[{ label: "Missing Link", target_page_id: null }]}
        onPageClick={vi.fn()}
      />,
    );
    const section = screen.getByLabelText("Linked pages");
    expect(within(section).getByText("Missing Link")).toBeInTheDocument();
    expect(within(section).queryByRole("button", { name: /Missing Link/ })).toBeNull();
    expect(within(section).getByText("Not organized yet")).toBeInTheDocument();
  });

  it("deduplicates backlinks by source page while keeping different pages with the same label", async () => {
    const user = userEvent.setup();
    render(
      <PageInfo
        sourceCount={0}
        sources={[]}
        inbound={[
          { source_page_id: "source-1", label: "First source" },
          { source_page_id: "source-1", label: "Duplicate source" },
          { source_page_id: "source-2", label: "Repeated label" },
          { source_page_id: "source-3", label: "Repeated label" },
        ]}
        revisions={[]}
        citations={undefined}
        citationState="none"
        onMemoryClick={vi.fn()}
        onPageClick={vi.fn()}
      />,
    );

    await user.click(screen.getByText(/Page info/i));
    expect(screen.getAllByRole("button", { name: "Repeated label" })).toHaveLength(2);
    expect(screen.getByRole("button", { name: "First source" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Duplicate source" })).not.toBeInTheDocument();
    expect(screen.getByText("3 backlinks · 0 revisions · 0 sources")).toBeInTheDocument();
  });
});
