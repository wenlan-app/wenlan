// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { allowsDevLiveCommand, allowsDevLiveRequest } from "./devLivePolicy";

describe("live dev read boundary", () => {
  it("permits graph reads and explicit read-only POST endpoints", () => {
    expect(allowsDevLiveRequest("GET", "/api/memory/graph")).toBe(true);
    expect(allowsDevLiveRequest("POST", "/api/memory/list")).toBe(true);
    expect(allowsDevLiveRequest("POST", "/api/brief")).toBe(true);
    expect(allowsDevLiveCommand("list_working_on")).toBe(true);
    expect(allowsDevLiveCommand("get_knowledge_graph_cmd")).toBe(true);
  });
  it("rejects writes including suffixes and encoded lookalikes", () => {
    for (const path of ["/api/memory/entities/archive", "/api/pages/search/delete", "/api/%70ages/search", "/api/config"]) {
      expect(allowsDevLiveRequest("POST", path)).toBe(false);
    }
    expect(allowsDevLiveRequest("DELETE", "/api/pages/example")).toBe(false);
    expect(allowsDevLiveRequest("PUT", "/api/config")).toBe(false);
    expect(allowsDevLiveCommand("create_page")).toBe(false);
    expect(allowsDevLiveCommand("set_space_status")).toBe(false);
  });
});
