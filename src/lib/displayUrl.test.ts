// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { displayUrl, hiddenUrlPrefixLength, isBareUrlLink } from "./displayUrl";

describe("displayUrl", () => {
  it("drops the scheme, www and a trailing slash", () => {
    expect(displayUrl("https://github.com/wenlan-app/wenlan")).toBe("github.com/wenlan-app/wenlan");
    expect(displayUrl("http://www.example.com/")).toBe("example.com");
    expect(displayUrl("https://wenlan.app/docs?x=1#top")).toBe("wenlan.app/docs?x=1#top");
  });

  it("measures only the hidden prefix", () => {
    expect(hiddenUrlPrefixLength("https://www.example.com/a")).toBe("https://www.".length);
    expect(hiddenUrlPrefixLength("http://example.com")).toBe("http://".length);
    expect(hiddenUrlPrefixLength("example.com")).toBe(0);
  });

  it("recognises bare-URL links but not labelled ones", () => {
    expect(isBareUrlLink("https://wenlan.app/docs", "https://wenlan.app/docs")).toBe(true);
    expect(isBareUrlLink("http://www.example.com", "www.example.com")).toBe(true);
    expect(isBareUrlLink("https://docs.turso.tech/libsql", "libSQL 的说明")).toBe(false);
    expect(isBareUrlLink("#concept:p-1", "#concept:p-1")).toBe(false);
  });
});
