// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import ContentRenderer from "./ContentRenderer";

const mermaid = vi.hoisted(() => ({
  initialize: vi.fn(),
  render: vi.fn(),
}));
vi.mock("mermaid", () => ({ default: mermaid }));

const PAGE = `The CLI calls the daemon.

\`\`\`mermaid
flowchart LR
  cli["CLI"] --> server["daemon"]
\`\`\`

Other code stays code.

\`\`\`sh
pnpm dev
\`\`\`
`;

describe("Mermaid diagrams in a page", () => {
  beforeEach(() => {
    mermaid.initialize.mockClear();
    mermaid.render.mockReset();
  });

  it("draws a mermaid block and leaves other code blocks as code", async () => {
    mermaid.render.mockResolvedValue({ svg: '<svg data-drawn="yes"></svg>' });
    const { container } = render(<ContentRenderer content={PAGE} variant="detail" />);

    await waitFor(() => expect(container.querySelector('svg[data-drawn="yes"]')).not.toBeNull());
    expect(mermaid.render).toHaveBeenCalledWith(
      expect.stringMatching(/^mermaid-/),
      'flowchart LR\n  cli["CLI"] --> server["daemon"]\n',
    );
    expect(mermaid.initialize).toHaveBeenCalledWith(
      expect.objectContaining({ securityLevel: "strict", startOnLoad: false }),
    );
    expect(screen.getByText("pnpm dev").closest("pre")).not.toBeNull();
    expect(container.textContent).not.toContain("flowchart LR");
  });

  it("lets a wide diagram shrink only to 80% of its own width", async () => {
    mermaid.render.mockResolvedValue({
      svg: '<svg id="x" width="100%" viewBox="0 0 1500 200" style="max-width: 1500px;"></svg>',
    });
    const { container } = render(<ContentRenderer content={PAGE} variant="detail" />);

    await waitFor(() => expect(container.querySelector("svg#x")).not.toBeNull());
    expect(container.querySelector("svg#x")?.getAttribute("style")).toBe(
      "min-width: 1200px; max-width: 1500px;",
    );
  });

  it("shows the diagram's text when Mermaid cannot draw it", async () => {
    mermaid.render.mockRejectedValue(new Error("Parse error"));
    const { container } = render(<ContentRenderer content={PAGE} variant="detail" />);

    await waitFor(() =>
      expect(container.querySelector("pre.page-diagram-source")?.textContent).toContain(
        'cli["CLI"] --> server["daemon"]',
      ),
    );
    expect(screen.queryByTestId("page-diagram")).toBeNull();
    expect(screen.getByText(/could not be drawn/)).toBeInTheDocument();
  });
});
