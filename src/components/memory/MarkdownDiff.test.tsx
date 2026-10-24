// SPDX-License-Identifier: AGPL-3.0-only
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import MarkdownDiff from "./MarkdownDiff";

const INS = { backgroundColor: "green" };
const DEL = { backgroundColor: "red" };

describe("MarkdownDiff", () => {
  it("renders a changed heading as a heading, without its #", () => {
    const { container } = render(
      <MarkdownDiff
        before={"# Reading position\n\nKept **bold** text."}
        after={"# Reading place\n\nKept **bold** text."}
        side="unified"
        insStyle={INS}
        delStyle={DEL}
      />,
    );
    const heading = container.querySelector("h1")!;
    expect(heading.textContent).toBe("Reading position place");
    expect(heading.querySelector("del[data-diff]")?.textContent).toBe("position");
    expect(heading.querySelector("ins[data-diff]")?.textContent).toBe("place");
    expect(container.textContent).not.toContain("#");
    expect(container.textContent).not.toContain("**");
    expect(container.querySelector("strong")?.textContent).toBe("bold");
  });

  it("marks words that run across bold without breaking it", () => {
    const { container } = render(
      <MarkdownDiff
        before="a b"
        after="a **new words** b"
        side="after"
        insStyle={INS}
        delStyle={DEL}
      />,
    );
    const strong = container.querySelector("strong")!;
    expect(strong.textContent).toBe("new words");
    expect(strong.querySelectorAll("ins[data-diff]").length).toBeGreaterThan(0);
    expect(container.textContent).not.toContain("**");
    expect(container.querySelector("del")).toBeNull();
  });

  it("keeps a strikethrough the page itself has unmarked", () => {
    const { container } = render(
      <MarkdownDiff before="~~old~~ x" after="~~old~~ x" side="unified" insStyle={INS} delStyle={DEL} />,
    );
    const del = container.querySelector("del")!;
    expect(del.textContent).toBe("old");
    expect(del.hasAttribute("data-diff")).toBe(false);
  });

  it("puts a new list item in the same list", () => {
    const { container } = render(
      <MarkdownDiff before={"- one\n- two"} after={"- one\n- two\n- three"} side="unified" insStyle={INS} delStyle={DEL} />,
    );
    expect(container.querySelectorAll("ul").length).toBe(1);
    expect(container.querySelectorAll("li").length).toBe(3);
    expect(container.querySelectorAll("li")[2].querySelector("ins[data-diff]")?.textContent).toBe("three");
  });
});
