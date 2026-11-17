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
// The person says yes to the app's own question before a skill is removed.
vi.mock("../../../lib/confirmAction", () => ({ confirmAction: vi.fn(() => Promise.resolve(true)) }));

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

  const openPreview = async (preview: object) => {
    tauri.searchSkillRegistry.mockResolvedValue({ via: "skills.sh", rows: [row("meeting-notes", "pass")] });
    tauri.previewSkill.mockResolvedValue(preview);
    render(wrap(<FindSkillsView />));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "meeting notes" } });
    fireEvent.submit(screen.getByRole("search"));
    fireEvent.click(await screen.findByRole("button", { name: /meeting-notes/ }));
  };
  const shown = {
    source: "o/r", skillId: "meeting-notes", commit: "0".repeat(40), repoPath: "", name: "meeting-notes", description: null,
    files: [{ path: "SKILL.md", text: "Read it all." }], runnable: [], safety: { verdict: "pass", audits: [] },
    sourceUrl: "https://github.com/o/r", skillsShUrl: "https://skills.sh/o/r/meeting-notes", installedAs: null,
  };

  it("UX-SKILL-13: says a file is cut, and will not install a skill that cannot be shown in full", async () => {
    await openPreview({
      ...shown,
      files: [{ path: "SKILL.md", text: "Read it all.", cut: false }, { path: "notes/long.md", text: "First lines", cut: true }],
      cannotInstall: "notes/long.md is too long to show in full (65536 characters at most), so Wenlan refuses this skill",
    });
    await screen.findByText("First lines");
    expect(screen.getAllByText("Cut here. The rest of this file is not shown.")).toHaveLength(1);
    const reason = screen.getByRole("note");
    expect(reason.textContent).toBe("notes/long.md is too long to show in full, so it can't be installed.");
    const install = screen.getByRole("button", { name: "Install" }) as HTMLButtonElement;
    expect(install.disabled).toBe(true);
    expect(install.getAttribute("aria-describedby")).toBe(reason.id);
    expect(screen.getByText("Can't be installed")).toBeTruthy();
    fireEvent.click(install);
    expect(tauri.installSkill).not.toHaveBeenCalled();
  });

  it("UX-SKILL-13: gives a reason it does not know as it came, and one for a file that is not text", async () => {
    await openPreview({ ...shown, files: [{ path: "SKILL.md", text: null }], cannotInstall: "SKILL.md is not readable text (not valid UTF-8), so Wenlan refuses this skill" });
    expect((await screen.findByRole("note")).textContent).toBe("SKILL.md can't be read as text, so it can't be installed.");
    expect((screen.getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("UX-SKILL-13: will not install a skill with characters that hide or reorder its text, and says which file", async () => {
    await openPreview({
      ...shown,
      files: [{ path: "SKILL.md", text: "Read it all.<U+E0073><U+E0065><U+E006E><U+E0064>", cut: false, marked: 4 }],
      cannotInstall: "SKILL.md holds characters that do not show or that reorder the text (the first is U+E0073), so Wenlan refuses this skill",
    });
    const reason = await screen.findByText("SKILL.md has characters that don't show or that reorder its text, so it can't be installed.");
    expect(reason.id).toBe("find-skills-blocked");
    const install = screen.getByRole("button", { name: "Install" }) as HTMLButtonElement;
    expect(install.disabled).toBe(true);
    expect(install.getAttribute("aria-describedby")).toBe(reason.id);
    fireEvent.click(install);
    expect(tauri.installSkill).not.toHaveBeenCalled();
  });

  it("UX-SKILL-12: shows each character that would not show as a visible mark, says how many, and still installs", async () => {
    await openPreview({
      ...shown,
      files: [
        { path: "SKILL.md", text: "Read<U+200B> it all<U+200D>.", cut: false, marked: 2 },
        { path: "notes.md", text: "Plain notes.", cut: false, marked: 0 },
      ],
      cannotInstall: null,
    });
    const marks = await screen.findAllByText(/^<U\+[0-9A-F]{4,6}>$/u);
    expect(marks.map((mark) => [mark.tagName, mark.textContent])).toEqual([["MARK", "<U+200B>"], ["MARK", "<U+200D>"]]);
    // The author's words around the marks are all still there, in order.
    expect(marks[0].closest("pre")?.textContent).toBe("Read<U+200B> it all<U+200D>.");
    expect(screen.getAllByText("2 characters in this file don't show on screen. Each is written out here as <U+…>.")).toHaveLength(1);
    expect(screen.getByText("Plain notes.").querySelector("mark")).toBeNull();
    expect((screen.getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("an answer without the cut mark and the refusal (a daemon older than the rule) shows no reason and leaves Install to the daemon", async () => {
    // Every current door answers with both fields: the daemon's route and
    // the desktop command run the same check, and both refuse the install
    // themselves. Only a browser page newer than its daemon sees neither
    // field; the page then blocks nothing and the daemon decides.
    await openPreview(shown);
    await screen.findByText("Read it all.");
    expect(screen.queryByRole("note")).toBeNull();
    expect(screen.queryByText("Cut here. The rest of this file is not shown.")).toBeNull();
    expect((screen.getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("UX-SKILL-13: says the daemon's refusal in plain words when an install, a take-in or a removal is refused", async () => {
    tauri.installSkill.mockRejectedValue(new Error("skills_need_this_computer: this Wenlan listens beyond this computer (WENLAN_BIND_ADDR), so skills cannot be installed, taken in or removed over it. Use the Wenlan app"));
    await openPreview(shown);
    await screen.findByText("Read it all.");
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    const refused = await screen.findByRole("alert");
    // A block of its own: what happened, then why, each on its own line.
    expect([...refused.querySelectorAll("p")].map((line) => line.textContent)).toEqual([
      "Couldn't install.",
      "Wenlan can be reached from other computers right now, so skills can't be changed here.",
    ]);
    expect(refused.textContent).not.toContain("WENLAN_BIND_ADDR");
  });

  it("UX-SKILL-13: Install is off while a refusal is shown under it", async () => {
    tauri.installSkill.mockRejectedValue(new Error("skills_need_the_person: only the person can install, take in or remove a skill. Ask them"));
    await openPreview(shown);
    await screen.findByText("Read it all.");
    const install = screen.getByRole("button", { name: "Install" }) as HTMLButtonElement;
    expect(install.disabled).toBe(false);
    fireEvent.click(install);
    const refused = await screen.findByRole("alert");
    expect(refused.textContent).toContain("Only you, at this computer, can install, take in or remove skills.");
    // Off for real, not only in looks, and it says why to a screen reader.
    expect(install.disabled).toBe(true);
    expect(install.getAttribute("aria-describedby")).toBe(refused.id);
    fireEvent.click(install);
    expect(tauri.installSkill).toHaveBeenCalledTimes(1);
    // Choosing again where it goes takes the refusal away; the button comes
    // back once the choice is complete.
    fireEvent.click(screen.getByLabelText(/One Space only/));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(install.disabled).toBe(true);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "Clinic" } });
    expect(install.disabled).toBe(false);
  });

  it("UX-SKILL-13: a refused take-in or removal says why, and a reason Wenlan has no words for comes through as it is", async () => {
    tauri.scanLocalSkills.mockResolvedValue([{ name: "my-review", path: "/h/.claude/skills/my-review", description: null }]);
    tauri.importLocalSkills.mockRejectedValue(new Error("skills_need_the_person: only the person can install, take in or remove a skill. Ask them"));
    tauri.listInstalledSkills.mockResolvedValue([{ name: "held", source: "o/r", skillId: "held", repoPath: "", commit: "c", hash: "h", verdict: "pass", scope: { kind: "all" }, description: null, installedAt: 1, updatedAt: 1, additions: "" }]);
    tauri.removeSkill.mockRejectedValue(new Error("no skill by that name"));
    render(wrap(<FindSkillsView />));
    fireEvent.click(await screen.findByRole("button", { name: "Take all into Wenlan" }));
    expect(await screen.findByText("Couldn't take them in: Only you, at this computer, can install, take in or remove skills.")).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Couldn't remove: no skill by that name")).toBeTruthy();
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
