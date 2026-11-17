// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Page } from "../../../lib/tauri";
import { folderSection, pageSection, sectionFolderPath, spaceLayout, topFolderIsASpaces } from "./pageSections";

const page = (overrides: Partial<Page>): Page => ({ id: "p", title: "p", summary: null, content: "", entity_id: null, domain: null, source_memory_ids: [], version: 1, status: "active", created_at: "2026-07-16", last_modified: "2026-07-16", last_compiled: "2026-07-16", ...overrides });

describe("page sections", () => {
  it("reads Rules and Skills from the folder a page already sits in, in any app language and at any depth", () => {
    for (const path of ["Rules", "work/rules", "規則", "筆記/规则/提交", "a\\RULES\\b"]) expect(folderSection(path)).toBe("rules");
    for (const path of ["Skills", "team/skills/writing", "技能"]) expect(folderSection(path)).toBe("skills");
    for (const path of ["", null, "Work", "My Rules", "Rules 2", "Skillset"]) expect(folderSection(path)).toBe("docs");
  });

  it("does not take a Space's own folder for a rules folder, as the daemon does not", () => {
    // A Space called Rules keeps its pages in a folder called Rules.
    const pages = [page({ id: "a", space: "Rules", storage_path: "Rules/plan.md" }), page({ id: "b", space: "Rules", storage_path: "Rules/Rules/git.md" })];
    const layout = spaceLayout(pages, true);
    expect(layout.spaceFolders).toEqual(["Rules"]);
    expect(pageSection(pages[0], layout)).toBe("docs");
    expect(pageSection(pages[1], layout)).toBe("rules");
    // The same for Skills, and for the folder a second Space of a name gets.
    const skills = spaceLayout([page({ space: "Skills", storage_path: "Skills 2/notes.md" })], true);
    expect(pageSection(page({ space: "Skills", storage_path: "Skills 2/notes.md" }), skills)).toBe("docs");
    expect(pageSection(page({ space: "Skills", storage_path: "Skills/notes.md" }), spaceLayout([page({ space: "Skills", storage_path: "Skills/notes.md" })], true))).toBe("docs");
    // With Space folders off no folder is a Space's: the name decides.
    expect(spaceLayout(pages, false).spaceFolders).toEqual([]);
    expect(pageSection(pages[0], spaceLayout(pages, false))).toBe("rules");
    // A page in no Space inside that folder is read by the folder's name.
    expect(pageSection(page({ storage_path: "Rules/loose.md" }), layout)).toBe("rules");
  });

  it("reads a team Space's Rules folder inside the team's folder, and never the team's or the Space's own name", () => {
    const teamSpaces = new Set(["Launch", "Rules"]);
    const pages = [
      page({ id: "a", space: "Launch", storage_path: "Acme/Launch/Rules/git.md" }),
      page({ id: "b", space: "Launch", storage_path: "Acme/Launch/plan.md" }),
      page({ id: "c", space: "Rules", storage_path: "Acme/Rules/plan.md" }),
      page({ id: "d", space: "Launch", storage_path: "Rules/Launch/plan.md" }),
    ];
    const layout = spaceLayout(pages, true, teamSpaces);
    expect([...layout.spaceFolders].sort()).toEqual(["Acme/Launch", "Acme/Rules", "Rules/Launch"]);
    expect([...layout.teamFolders].sort()).toEqual(["Acme", "Rules"]);
    expect(pages.map((item) => pageSection(item, layout))).toEqual(["rules", "docs", "docs", "docs"]);
  });

  it("counts every page in the section that lists it: no page is in one section's count and another's list", () => {
    const pages = [
      page({ id: "a", space: "Rules", storage_path: "Rules/plan.md" }),
      page({ id: "b", space: "Skills", storage_path: "Skills/plan.md" }),
      page({ id: "c", storage_path: "Rules/git.md" }),
      page({ id: "d", space: "Work", storage_path: "Work/Rules/git.md" }),
    ];
    const layout = spaceLayout(pages, true);
    expect(pages.map((item) => pageSection(item, layout))).toEqual(["docs", "docs", "rules", "rules"]);
  });

  it("puts every page in exactly one section: a How I work page is a rules page wherever its file is", () => {
    expect(pageSection(page({ storage_path: "Work/plan.md" }))).toBe("docs");
    expect(pageSection(page({ storage_path: "plan.md" }))).toBe("docs");
    expect(pageSection(page({}))).toBe("docs");
    expect(pageSection(page({ storage_path: "Rules/git.md" }))).toBe("rules");
    expect(pageSection(page({ storage_path: "Skills/release-notes.md" }))).toBe("skills");
    expect(pageSection(page({ storage_path: "Work/how.md", title: "How I work — Wenlan" }))).toBe("rules");
    expect(pageSection(page({ status: "draft", folder_path: "規則" }))).toBe("rules");
  });

  it("finds the section's folder for a new page, nearest the top first, and never one inside a Space's or a team's folder", () => {
    expect(sectionFolderPath("rules", ["Work", "Work/Rules", "規則"])).toBe("規則");
    expect(sectionFolderPath("skills", ["Work", "Rules"])).toBeNull();
    expect(sectionFolderPath("skills", ["a/Skills"])).toBe("a/Skills");
    const layout = spaceLayout([page({ space: "Work", storage_path: "Work/Rules/git.md" }), page({ space: "Launch", storage_path: "Acme/Launch/plan.md" }), page({ space: "Rules", storage_path: "Rules/plan.md" })], true, new Set(["Launch"]));
    // A Space's Rules folder gives rules to that Space only; a Space called
    // Rules has a folder that is no rules folder at all.
    expect(sectionFolderPath("rules", ["Work", "Work/Rules", "Acme/Launch/Rules", "Rules"], layout)).toBeNull();
    expect(sectionFolderPath("rules", ["Work/Rules", "Notes/Rules"], layout)).toBe("Notes/Rules");
    expect(topFolderIsASpaces("rules", layout)).toBe(true);
    expect(topFolderIsASpaces("Acme", layout)).toBe(true);
    expect(topFolderIsASpaces("Skills", layout)).toBe(false);
  });
});
