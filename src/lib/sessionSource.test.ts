// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { AgentSession } from "./tauri";
import {
  formatSessionClock,
  formatSessionDay,
  sessionPlaceName,
  sessionRepoName,
  sessionResumeCommand,
  sessionToolActionLabel,
  sessionToolLabel,
  shortCommit,
} from "./sessionSource";

const session = (over: Partial<AgentSession> = {}): AgentSession => ({
  id: "ses_1",
  tool: "claude-code",
  external_id: "abc-123",
  first_write_at: 1,
  last_write_at: 2,
  ...over,
});

describe("sessionToolLabel", () => {
  it.each([
    ["claude-code", "Claude Code"],
    ["codex", "ChatGPT / Codex"],
    ["opencode", "OpenCode"],
    ["gemini-cli", "Gemini CLI"],
    ["chatgpt", "ChatGPT"],
    ["claude-ai", "Claude"],
    ["some-new-tool", "Some New Tool"],
  ])("%s shows as %s", (tool, label) => {
    expect(sessionToolLabel(tool)).toBe(label);
  });
});

describe("sessionToolActionLabel", () => {
  it("names the executable an action targets", () => {
    expect(sessionToolActionLabel("codex")).toBe("Codex");
    expect(sessionToolActionLabel("claude-code")).toBe("Claude Code");
  });
});

describe("where a session ran", () => {
  it("names the repository from its root folder, on any path style", () => {
    expect(sessionRepoName(session({ repo_root: "/work/wenlan" }))).toBe("wenlan");
    expect(sessionRepoName(session({ repo_root: "C:\\work\\wenlan\\" }))).toBe("wenlan");
  });

  it("falls back to the remote's name", () => {
    expect(sessionRepoName(session({ repo_remote: "git@github.com:wenlan-app/wenlan.git" }))).toBe("wenlan");
    expect(sessionRepoName(session({ repo_remote: "https://github.com/wenlan-app/wenlan/" }))).toBe("wenlan");
  });

  it("prefers the repository, then the Space, then nothing", () => {
    expect(sessionPlaceName(session({ repo_root: "/work/wenlan", space: "Work" }))).toBe("wenlan");
    expect(sessionPlaceName(session({ space: "Work" }))).toBe("Work");
    expect(sessionPlaceName(session({ space: "  " }))).toBeNull();
    expect(sessionPlaceName(session())).toBeNull();
  });

  it("shortens a commit to seven characters", () => {
    expect(shortCommit("be0283ac71013796e7674387f78b45b82b01d54e")).toBe("be0283a");
    expect(shortCommit(" ")).toBeNull();
    expect(shortCommit(undefined)).toBeNull();
  });
});

describe("sessionResumeCommand", () => {
  it("builds the resume command for the tools that have one", () => {
    expect(sessionResumeCommand("claude-code", "00893aaf-19fa")).toBe("claude --resume 00893aaf-19fa");
    expect(sessionResumeCommand("codex", "019a.b_c-d")).toBe("codex resume 019a.b_c-d");
  });

  it("offers nothing for other tools or for ids that are not plain", () => {
    expect(sessionResumeCommand("opencode", "abc")).toBeNull();
    expect(sessionResumeCommand("chatgpt", "abc")).toBeNull();
    expect(sessionResumeCommand("claude-code", "abc; rm -rf ~")).toBeNull();
    expect(sessionResumeCommand("claude-code", "")).toBeNull();
  });
});

describe("session dates", () => {
  const local = (year: number) => new Date(year, 9, 20, 9, 12).getTime() / 1000;

  it("leaves the year off for this year and adds it otherwise", () => {
    const now = new Date(2026, 5, 1);
    expect(formatSessionDay(local(2026), "en", now)).toBe("Oct 20");
    expect(formatSessionDay(local(2025), "en", now)).toContain("2025");
  });

  it("writes the day in the reader's language", () => {
    expect(formatSessionDay(local(2026), "zh-Hant", new Date(2026, 5, 1))).toBe("10月20日");
  });

  it("formats the clock time", () => {
    expect(formatSessionClock(local(2026), "zh-Hant")).toMatch(/(?<!\d)9:12/);
  });
});
