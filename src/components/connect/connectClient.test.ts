// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpClient } from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  installClientPlugin: vi.fn().mockResolvedValue(undefined),
  installClientSkills: vi.fn().mockResolvedValue("/home/u/.cursor/skills"),
  writeMcpConfig: vi.fn().mockResolvedValue([]),
}));

import { installClientPlugin, installClientSkills, writeMcpConfig } from "../../lib/tauri";
import { connectClient, needsSkillsOnly } from "./connectClient";

const yes = { kind: "yes" } as const;
const no = { kind: "no" } as const;

function client(overrides: Partial<McpClient>): McpClient {
  return {
    name: "Cursor",
    client_type: "cursor",
    config_path: "/home/u/.cursor/mcp.json",
    detected: yes,
    already_configured: no,
    has_raw_entry: no,
    has_raw_duplicate: no,
    has_plugin: no,
    has_skills: no,
    ...overrides,
  };
}

describe("connectClient", () => {
  beforeEach(() => vi.clearAllMocks());

  it("writes the entry and then the skills for a tool with a skills folder", async () => {
    await connectClient(client({}), "setup");
    expect(writeMcpConfig).toHaveBeenCalledWith("cursor");
    expect(installClientSkills).toHaveBeenCalledWith("cursor");
    expect(installClientPlugin).not.toHaveBeenCalled();
  });

  it("only adds the skills when the entry is already there and healthy", async () => {
    const connected = client({ has_raw_entry: yes, already_configured: yes });
    expect(needsSkillsOnly(connected)).toBe(true);
    await connectClient(connected, "setup");
    expect(writeMcpConfig).not.toHaveBeenCalled();
    expect(installClientSkills).toHaveBeenCalledWith("cursor");
  });

  it("does not reinstall skills that are already there", async () => {
    await connectClient(client({ has_skills: yes }), "setup");
    expect(writeMcpConfig).toHaveBeenCalledWith("cursor");
    expect(installClientSkills).not.toHaveBeenCalled();
  });

  it("never writes skills for a tool without a skills folder", async () => {
    await connectClient(client({ client_type: "claude_desktop", has_skills: null }), "setup");
    expect(writeMcpConfig).toHaveBeenCalledWith("claude_desktop");
    expect(installClientSkills).not.toHaveBeenCalled();
  });

  it("keeps the plugin route for Claude Code and Codex", async () => {
    await connectClient(client({ client_type: "codex_cli", has_skills: null }), "setup");
    expect(installClientPlugin).toHaveBeenCalledWith("codex_cli");
    expect(writeMcpConfig).not.toHaveBeenCalled();
    expect(installClientSkills).not.toHaveBeenCalled();
  });

  it("a skills failure fails the whole setup", async () => {
    vi.mocked(installClientSkills).mockRejectedValueOnce(new Error("folder not ours"));
    await expect(connectClient(client({}), "setup")).rejects.toThrow("folder not ours");
  });
});
