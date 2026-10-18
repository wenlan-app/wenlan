// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Page } from "../../lib/tauri";
import PageSearchResult, { pageSearchSnippet } from "./PageSearchResult";

describe("page search previews", () => {
  it("shows the literal body match even when it is beyond the opening paragraph", () => {
    const snippet = pageSearchSnippet({
      summary: "A maintenance guide.",
      content: `${"Unrelated introduction. ".repeat(30)}\n## Recovery\nThe **rollback** preserves [source history](https://example.com) [1].`,
    }, "rollback");
    expect(snippet).toContain("The rollback preserves source history.");
    expect(snippet).toMatch(/^…/);
    expect(snippet).not.toMatch(/\*\*|https:|\[1\]|##/);
    expect(snippet.length).toBeLessThanOrEqual(242);
  });

  it("falls back to an existing summary, then body, without inventing a reason", () => {
    expect(pageSearchSnippet({ summary: "A real summary.", content: "Body text." }, "semantic query"))
      .toBe("A real summary.");
    expect(pageSearchSnippet({ summary: "  ", content: "Pages preserve [[Source history|sources]] and `page_write`." }, "semantic query"))
      .toBe("Pages preserve sources and page_write.");
    expect(pageSearchSnippet({ summary: null, content: "  " }, "query")).toBe("");
  });

  it.each([
    ["Read items[0] before items[1] [2].", "Read items[0] before items[1]."],
    ["Read `items[0]` before `items[1]` [1].", "Read items[0] before items[1]."],
    ["```ts\nconst first = items[0];\n```", "const first = items[0];"],
    ["~~~~ts\nitems[1] = [2](url);\n~~~~~\nSee [2](#citation:2).", "items[1] = [2](url); See."],
    ["Use ``items[0] + `value` + **literal**`` [3].", "Use items[0] + `value` + **literal**."],
    ["    items[0] = [[value]];", "items[0] = [[value]];"],
    ["```ts\nitems[0] = [1];", "items[0] = [1];"],
  ])("preserves literal code in a preview: %s", (content, expected) => {
    expect(pageSearchSnippet({ summary: null, content }, "items[0]")).toBe(expected);
    expect(pageSearchSnippet({ summary: content, content: "" }, "items[0]")).toBe(expected);
  });

  it("prefers a full phrase over a partial match in either field", () => {
    expect(pageSearchSnippet({
      summary: "Original guide.",
      content: "Rollback preserves original source.",
    }, "rollback preserves original source")).toBe("Rollback preserves original source.");
    expect(pageSearchSnippet({
      summary: "Rollback preserves original source.",
      content: "Original guide.",
    }, "rollback preserves original source")).toBe("Rollback preserves original source.");
  });

  it("does not split an emoji at the excerpt's starting boundary", () => {
    const content = `😀${"x".repeat(59)}needle`;
    expect(pageSearchSnippet({ summary: null, content }, "needle")).toBe(content);
  });

  it("keeps Chinese matches, literal punctuation and safe text rendering", () => {
    expect(pageSearchSnippet({ summary: null, content: `${"來源與頁面一起保存。".repeat(40)}回復時保留來源。` }, "回復"))
      .toContain("回復時保留來源。");
    expect(pageSearchSnippet({ summary: "Use C++ for this module.", content: "" }, "C++"))
      .toBe("Use C++ for this module.");
    expect(pageSearchSnippet({ summary: null, content: `${"İ".repeat(400)} rollback preserves sources.` }, "rollback"))
      .toContain("rollback preserves sources.");
    expect(pageSearchSnippet({ summary: null, content: "page_write keeps source_agent when count < 10 and size > 3." }, "source_agent"))
      .toBe("page_write keeps source_agent when count < 10 and size > 3.");
  });

  it("opens through click, Enter and Space, without nested snippet links", async () => {
    const onClick = vi.fn();
    const user = userEvent.setup();
    render(<PageSearchResult page={{ title: "Recovery", summary: null, content: "Use [rollback](https://example.com)." } as Page} query="rollback" onClick={onClick} />);
    const row = screen.getByRole("button");
    expect(row).toHaveTextContent("Use rollback.");
    expect(row.querySelector("mark")).toHaveTextContent("rollback");
    expect(row.querySelector("a")).toBeNull();
    await user.click(row);
    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    expect(onClick).toHaveBeenCalledTimes(3);
  });

  it("renders a title-only result without an empty snippet slot", () => {
    render(<PageSearchResult page={{ title: "New page", summary: null, content: "" } as Page} query="anything" onClick={vi.fn()} />);
    const row = screen.getByRole("button", { name: "New page" });
    expect(row.children).toHaveLength(1);
  });
});
