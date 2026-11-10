// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { originalProse, readablePageContent } from "./originalContent";
import { pagePreview } from "../../pages/NewPagesList";
import { pageLinkPreviewExcerpt } from "../PageLinkPreview";

const HTML = "<!doctype html>\n<html><body><h1>Release map</h1></body></html>\n";
const SAVED = `Saved from a Claude Code session, 2026-11-08 18:30 UTC. Original file: \`originals/claude-code/s-1/a.html\`.\n\n\`\`\`html\n${HTML}\`\`\`\n`;
const KEPT = "Kept from the Claude chat \"Plan\" ([open in Claude](https://claude.ai/chat/c-1)), written 2026-11-08 18:30 UTC. Original file: `originals/claude/c-1/a.md`.\n\n# Plan\n\nShip the release on Friday.\n";

function original(content: string) {
  return { title: "Release map", summary: null, content, creation_kind: "original" };
}

describe("an original's summary on cards and previews", () => {
  it("never shows where it came from or the HTML it keeps", () => {
    expect(originalProse(SAVED)).toBe("");
    expect(pagePreview(original(SAVED))).toBe("");
    expect(pageLinkPreviewExcerpt(original(SAVED))).toBe("");
  });

  it("shows the notes added after an HTML original", () => {
    const content = `${SAVED}\nThe launch moved to May.\n`;
    expect(pagePreview(original(content))).toBe("The launch moved to May.");
    expect(pageLinkPreviewExcerpt(original(content))).toBe("The launch moved to May.");
  });

  it("shows a Markdown original's own words, not the line naming its file", () => {
    expect(pagePreview({ ...original(KEPT), title: "Plan" })).toBe("Ship the release on Friday.");
    const excerpt = pageLinkPreviewExcerpt(original(KEPT));
    expect(excerpt).toContain("Ship the release on Friday.");
    expect(excerpt).not.toMatch(/originals\/|Kept from/);
  });

  it("skips kept code and the line saying an AI tool saved it", () => {
    const content = "Saved by an AI tool, 2026-11-08 18:30 UTC. Original file: `originals/x/y/z.svg`.\n\n```python\nprint(1)\n```\n\nA note.";
    expect(pagePreview(original(content))).toBe("A note.");
  });

  it("leaves pages that are not originals as they are", () => {
    const content = "Saved from the fire, the letters were dried.\n\nMore.";
    expect(readablePageContent({ content, creation_kind: "distilled" })).toBe(content);
    expect(pagePreview({ title: "Letters", summary: null, content, creation_kind: "distilled" }))
      .toBe("Saved from the fire, the letters were dried.");
  });
});
