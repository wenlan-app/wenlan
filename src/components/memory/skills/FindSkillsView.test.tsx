// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const tauri = vi.hoisted(() => ({
  searchSkillRegistry: vi.fn(),
  previewSkill: vi.fn(),
  installSkill: vi.fn(),
  listInstalledSkills: vi.fn(),
  listSpaces: vi.fn(),
  refreshInstalledSkills: vi.fn(),
  removeSkill: vi.fn(),
  setSkillAdditions: vi.fn(),
  scanLocalSkills: vi.fn(),
  importLocalSkills: vi.fn(),
  LOCAL_SKILL_SOURCE: "local",
}));
vi.mock("../../../lib/tauri", () => tauri);
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn(() => Promise.resolve()) }));

import { FindSkillsView, visibleRows } from "./FindSkillsView";

const row = (skillId: string, verdict: "pass" | "warn" | "fail") => ({ source: "o/r", skillId, name: skillId, installs: 5, verdict });

function wrap(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{node}</QueryClientProvider>;
}

describe("visibleRows", () => {
  it("lists passing skills, keeps warnings behind the toggle and never shows failures", () => {
    const rows = [row("a", "pass"), row("b", "warn"), row("c", "fail")];
    expect(visibleRows(rows, false)).toEqual({ shown: [rows[0]], hidden: 1 });
    expect(visibleRows(rows, true)).toEqual({ shown: [rows[0], rows[1]], hidden: 0 });
  });
});

describe("FindSkillsView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauri.listInstalledSkills.mockResolvedValue([]);
    tauri.listSpaces.mockResolvedValue([{ id: "s1", name: "Clinic" }]);
    tauri.refreshInstalledSkills.mockResolvedValue([]);
    tauri.scanLocalSkills.mockResolvedValue([]);
  });

  it("takes skills already on this computer into Wenlan in one click", async () => {
    tauri.scanLocalSkills.mockResolvedValueOnce([
      { name: "my-review", path: "/h/.claude/skills/my-review", description: "How I review." },
      { name: "deploy", path: "/h/.claude/skills/deploy", description: null },
    ]).mockResolvedValue([]);
    tauri.importLocalSkills.mockResolvedValue({ imported: ["my-review"], failed: [["deploy", "too large"]] });
    render(wrap(<FindSkillsView />));
    expect(await screen.findByText("my-review")).toBeTruthy();
    expect(screen.getByText(/The originals stay as they are/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Take all into Wenlan" }));
    await waitFor(() => expect(tauri.importLocalSkills).toHaveBeenCalledWith(["my-review", "deploy"]));
    expect(await screen.findByText("Took in 1 skill.")).toBeTruthy();
    expect(screen.getByText("Couldn't take in deploy.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Take all into Wenlan" })).toBeNull();
  });

  it("searches, opens a skill with its full text, and installs it into one Space at the pinned commit", async () => {
    tauri.searchSkillRegistry.mockResolvedValue({ via: "skills.sh", rows: [row("meeting-notes", "pass"), row("risky", "warn")] });
    const preview = {
      source: "o/r", skillId: "meeting-notes", commit: "0".repeat(40), repoPath: "skills/meeting-notes",
      name: "meeting-notes", description: "Turn transcripts into decisions.",
      files: [{ path: "SKILL.md", text: "---\nname: meeting-notes\n---\nRead it all." }],
      runnable: [], safety: { verdict: "pass", audits: [{ provider: "Snyk", status: "pass", summary: "" }] },
      sourceUrl: "https://github.com/o/r", skillsShUrl: "https://skills.sh/o/r/meeting-notes", installedAs: null,
    };
    tauri.previewSkill.mockResolvedValue(preview);
    tauri.installSkill.mockResolvedValue({ skill: {}, tools: ["/h/.claude/skills/meeting-notes"], skipped: [] });

    render(wrap(<FindSkillsView />));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "meeting notes" } });
    fireEvent.submit(screen.getByRole("search"));
    await screen.findByText("meeting-notes");
    expect(screen.queryByText("risky")).toBeNull();
    expect(screen.getByText("1 with warnings not shown")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /meeting-notes/ }));
    await screen.findByText(/Read it all\./);
    expect(screen.getByText("Text only, no programs")).toBeTruthy();

    fireEvent.click(screen.getByLabelText(/One Space only/));
    const install = screen.getByRole("button", { name: "Install" }) as HTMLButtonElement;
    expect(install.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Choose a Space"), { target: { value: "Clinic" } });
    fireEvent.click(install);
    await waitFor(() => expect(tauri.installSkill).toHaveBeenCalledWith(preview, { kind: "space", name: "Clinic" }));
    await screen.findByText("Installed for 1 AI tool");
  });

  it("asks before installing a skill whose checks warned", async () => {
    tauri.searchSkillRegistry.mockResolvedValue({ via: "wenlan.app", rows: [row("risky", "warn")] });
    tauri.previewSkill.mockResolvedValue({
      source: "o/r", skillId: "risky", commit: "1".repeat(40), repoPath: "", name: "risky", description: null,
      files: [{ path: "SKILL.md", text: "x" }, { path: "run.sh", text: "#!/bin/sh" }], runnable: ["run.sh"],
      safety: { verdict: "warn", audits: [{ provider: "Snyk", status: "warn", summary: "1 issue" }] },
      sourceUrl: "https://github.com/o/r", skillsShUrl: "https://skills.sh/o/r/risky", installedAs: null,
    });
    render(wrap(<FindSkillsView />));
    fireEvent.click(screen.getByLabelText("Also show ones with warnings"));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "risky" } });
    fireEvent.submit(screen.getByRole("search"));
    fireEvent.click(await screen.findByRole("button", { name: /risky/ }));
    await screen.findByText("Snyk: 1 issue");
    expect(screen.getByText(/Wenlan never runs them/)).toBeTruthy();
    const install = screen.getByRole("button", { name: "Install" }) as HTMLButtonElement;
    expect(install.disabled).toBe(true);
    fireEvent.click(screen.getByLabelText(/Install even though/));
    expect(install.disabled).toBe(false);
  });
});
