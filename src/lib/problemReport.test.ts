// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { issueUrl } from "./problemReport";

describe("issueUrl", () => {
  it("prefills the title and body", () => {
    const url = new URL(issueUrl("Could not reach the background service", "- Version: 0.20.6"));
    expect(url.origin + url.pathname).toBe("https://github.com/wenlan-app/wenlan/issues/new");
    expect(url.searchParams.get("title")).toBe("Could not reach the background service");
    expect(url.searchParams.get("body")).toBe("- Version: 0.20.6");
  });

  it("drops the oldest log lines first when the body is too long", () => {
    const lines = Array.from({ length: 400 }, (_, i) => `line ${i} ${"x".repeat(40)}`);
    const body = `- Version: 0.20.6\n\n\`\`\`\n${lines.join("\n")}\n\`\`\`\n`;
    const url = issueUrl("t", body);
    expect(url.length).toBeLessThanOrEqual(7000);
    const kept = new URL(url).searchParams.get("body")!;
    expect(kept).toContain("- Version: 0.20.6");
    expect(kept).toContain("line 399");
    expect(kept).not.toContain("line 0 ");
  });
});
