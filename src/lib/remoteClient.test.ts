// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { clientIdentity } from "./remoteClient";

describe("clientIdentity", () => {
  it.each([
    ["claude.ai", "Claude"],
    ["claude.com", "Claude"],
    ["chatgpt.com", "ChatGPT"],
    ["chat.openai.com", "ChatGPT"],
    ["agent.meta.ai", "Muse"],
    ["CLAUDE.AI", "Claude"],
  ])("names %s as %s when the relay agrees", (redirectHost, name) => {
    expect(clientIdentity({ knownClient: true, redirectHost })).toEqual({ kind: "known", name });
  });

  it("never promotes a host the app does not list, whatever the relay says", () => {
    for (const redirectHost of ["evil.example", "claude.ai.evil.example", "notclaude.ai", "claude.ai:8443", "chatgpt.com.attacker.test", "meta.ai", "evil.agent.meta.ai"]) {
      expect(clientIdentity({ knownClient: true, redirectHost })).toEqual({
        kind: "unknown", host: redirectHost,
      });
    }
  });

  it("does not name a listed host when the relay did not call it known", () => {
    expect(clientIdentity({ knownClient: false, redirectHost: "claude.ai" })).toEqual({ kind: "unknown", host: "claude.ai" });
    expect(clientIdentity({ redirectHost: "claude.ai" })).toEqual({ kind: "unknown", host: "claude.ai" });
  });

  it("treats an older relay answer without identity as an unrecognized app", () => {
    expect(clientIdentity({})).toEqual({ kind: "unknown", host: null });
    expect(clientIdentity({ knownClient: null, redirectHost: "  " })).toEqual({ kind: "unknown", host: null });
  });
});
