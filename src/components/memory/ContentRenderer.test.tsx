// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, it, expect } from "vitest";
import { act, render, screen } from "@testing-library/react";
import ContentRenderer from "./ContentRenderer";
import { resetAttachmentsDirForTest } from "../../lib/pageAttachments";

describe("ContentRenderer page attachments", () => {
  afterEach(() => resetAttachmentsDirForTest(null));

  it("shows images and videos from the pages folder and leaves web images alone", () => {
    resetAttachmentsDirForTest("/pages/_attachments");
    const content = "![Home](_attachments/home-1.png)\n\n![Film](_attachments/film-1.mp4)\n\n![Web](https://wenlan.app/a.png)";
    const { container } = render(<ContentRenderer content={content} variant="detail" />);
    expect(screen.getByAltText("Home")).toHaveAttribute("src", "asset://localhost//pages/_attachments/home-1.png");
    const video = container.querySelector("video");
    expect(video).toHaveAttribute("src", "asset://localhost//pages/_attachments/film-1.mp4#t=0.1");
    expect(video).toHaveAttribute("aria-label", "Film");
    expect(screen.getByAltText("Web")).toHaveAttribute("src", "https://wenlan.app/a.png");
  });

  it("shows the alt text until the folder is known", () => {
    resetAttachmentsDirForTest(null);
    render(<ContentRenderer content="![Home](_attachments/home-1.png)" variant="detail" />);
    expect(screen.getByText("Home")).toBeInTheDocument();
    act(() => resetAttachmentsDirForTest("/pages/_attachments"));
    expect(screen.getByAltText("Home")).toHaveAttribute("src", "asset://localhost//pages/_attachments/home-1.png");
  });
});

describe("ContentRenderer detail variant", () => {
  it("renders plain text as a paragraph", () => {
    render(<ContentRenderer content="Lucian prefers TDD" variant="detail" />);
    expect(screen.getByText("Lucian prefers TDD")).toBeInTheDocument();
  });

  it("renders key-value content with bold labels", () => {
    const content = "Context: auth rewrite\nDecision: use JWT";
    const { container } = render(
      <ContentRenderer content={content} variant="detail" />
    );
    const strongs = container.querySelectorAll("strong");
    expect(strongs.length).toBeGreaterThanOrEqual(2);
    expect(strongs[0].textContent).toBe("Context:");
    expect(strongs[1].textContent).toBe("Decision:");
  });

  it("renders list content as list elements", () => {
    const content = "- Use libSQL\n- Keep local-first\n- AGPL license";
    const { container } = render(
      <ContentRenderer content={content} variant="detail" />
    );
    const listItems = container.querySelectorAll("li");
    expect(listItems.length).toBe(3);
    expect(listItems[0].textContent).toBe("Use libSQL");
  });

  it("renders URLs as clickable links", () => {
    const content = "Check https://example.com for details";
    const { container } = render(
      <ContentRenderer content={content} variant="detail" />
    );
    const link = container.querySelector("a");
    expect(link).not.toBeNull();
    expect(link!.getAttribute("href")).toBe("https://example.com");
  });

  it("opens links in external browser with target=_blank", () => {
    const content = "See https://example.com";
    const { container } = render(
      <ContentRenderer content={content} variant="detail" />
    );
    const link = container.querySelector("a");
    expect(link!.getAttribute("target")).toBe("_blank");
    expect(link!.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("shows empty placeholder for empty content", () => {
    render(<ContentRenderer content="" variant="detail" />);
    expect(screen.getByText("No content")).toBeInTheDocument();
  });
});

describe("ContentRenderer card variant", () => {
  it("renders single-fact as plain text", () => {
    render(<ContentRenderer content="Lucian prefers TDD" variant="card" />);
    expect(screen.getByText("Lucian prefers TDD")).toBeInTheDocument();
  });

  it("renders first key-value pair with bold key for key-value shape", () => {
    const content = "Context: auth rewrite\nDecision: use JWT";
    const { container } = render(
      <ContentRenderer content={content} variant="card" />
    );
    const strong = container.querySelector("strong");
    expect(strong).not.toBeNull();
    expect(strong!.textContent).toBe("Context:");
  });

  it("renders first list item for list content", () => {
    const content = "- Use libSQL\n- Keep local-first";
    render(<ContentRenderer content={content} variant="card" />);
    expect(screen.getByText("Use libSQL")).toBeInTheDocument();
  });

  it("renders first sentence for prose content", () => {
    const content =
      "We decided to use libSQL for storage. It supports vectors natively.";
    render(<ContentRenderer content={content} variant="card" />);
    expect(
      screen.getByText("We decided to use libSQL for storage.")
    ).toBeInTheDocument();
  });

  it("does not render react-markdown elements for card variant", () => {
    const content = "- Use libSQL\n- Keep local-first";
    const { container } = render(
      <ContentRenderer content={content} variant="card" />
    );
    expect(container.querySelectorAll("li").length).toBe(0);
  });

  it("shows empty placeholder for empty content", () => {
    render(<ContentRenderer content="" variant="card" />);
    expect(screen.getByText("No content")).toBeInTheDocument();
  });
});

describe("ContentRenderer citation links", () => {
  it("renders #citation: links through renderCitation", () => {
    render(
      <ContentRenderer
        content="A claim.[1](#citation:1) More text."
        variant="detail"
        renderCitation={(k) => <button data-testid={`chip-${k}`}>chip {k}</button>}
      />,
    );
    expect(screen.getByTestId("chip-1")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "1" })).toBeNull();
  });

  it("leaves ordinary links alone when renderCitation is set", () => {
    const { container } = render(
      <ContentRenderer
        content="See [docs](https://example.com) and a claim.[1](#citation:1)"
        variant="detail"
        renderCitation={(k) => <span data-testid={`chip-${k}`} />}
      />,
    );
    const link = container.querySelector('a[href="https://example.com"]');
    expect(link).not.toBeNull();
    expect(link!.getAttribute("target")).toBe("_blank");
  });

  it("renders #citation: hrefs as plain anchors when renderCitation is absent", () => {
    const { container } = render(
      <ContentRenderer content="A claim.[1](#citation:1)" variant="detail" />,
    );
    expect(container.querySelector('a[href="#citation:1"]')).not.toBeNull();
  });

  it("gives canonical page links a visible type cue", () => {
    const { container } = render(
      <ContentRenderer content="See [the city plan](#concept:page-17)." variant="detail" />,
    );
    const link = container.querySelector('a[href="#concept:page-17"]');
    expect(link).toHaveAttribute("data-reference-kind", "page");
    expect(link?.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });
});
