// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { leadingMarkdownH1MatchesTitle, withoutLeadingTitleH1 } from "./pageEditorPresentation";

describe("leadingMarkdownH1MatchesTitle", () => {
  it("matches the editable leading H1 to the Page title without rewriting source", () => {
    expect(
      leadingMarkdownH1MatchesTitle(
        "# Native Editor Review\n",
        "Native Editor Review",
      ),
    ).toBe(true);
    expect(
      leadingMarkdownH1MatchesTitle(
        "\uFEFF\n#  Native   Editor Review  ##\r\n",
        "Native Editor Review",
      ),
    ).toBe(true);
  });

  it("does not treat a different Markdown construct as the Page title", () => {
    expect(
      leadingMarkdownH1MatchesTitle(
        "## Native Editor Review\n",
        "Native Editor Review",
      ),
    ).toBe(false);
    expect(
      leadingMarkdownH1MatchesTitle(
        "Native Editor Review\n====================\n",
        "Native Editor Review",
      ),
    ).toBe(false);
    expect(
      leadingMarkdownH1MatchesTitle(
        "Paragraph\n\n# Native Editor Review\n",
        "Native Editor Review",
      ),
    ).toBe(false);
  });

  it("keeps comparison case-sensitive and source-visible", () => {
    expect(
      leadingMarkdownH1MatchesTitle("# Different\n", "Native Editor Review"),
    ).toBe(false);
    expect(
      leadingMarkdownH1MatchesTitle(
        "# native editor review\n",
        "Native Editor Review",
      ),
    ).toBe(false);
    expect(
      leadingMarkdownH1MatchesTitle(
        "# **Native Editor Review**\n",
        "Native Editor Review",
      ),
    ).toBe(false);
  });

  it("rejects empty titles and code-indented hashes", () => {
    expect(leadingMarkdownH1MatchesTitle("# \n", "")).toBe(false);
    expect(
      leadingMarkdownH1MatchesTitle(
        "    # Native Editor Review\n",
        "Native Editor Review",
      ),
    ).toBe(false);
  });
});

describe("withoutLeadingTitleH1", () => {
  it("drops a first H1 that repeats the title, with its blank lead-in and BOM", () => {
    expect(withoutLeadingTitleH1("# Trip plan\n\nBody", "Trip plan")).toBe("\nBody");
    expect(withoutLeadingTitleH1("﻿\n  # Trip plan #\r\nBody", "Trip plan")).toBe("Body");
    expect(withoutLeadingTitleH1("# Trip plan", "Trip plan")).toBe("");
  });

  it("leaves a different first heading and later H1s as written", () => {
    expect(withoutLeadingTitleH1("# Another heading\nBody", "Trip plan")).toBe("# Another heading\nBody");
    expect(withoutLeadingTitleH1("Intro\n# Trip plan\n", "Trip plan")).toBe("Intro\n# Trip plan\n");
    expect(withoutLeadingTitleH1("## Trip plan\nBody", "Trip plan")).toBe("## Trip plan\nBody");
  });
});
