// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", () => ({
  dismissRepoDocFix: vi.fn(),
}));

import type { RepoDocFix, RepoWikiBinding } from "../../../lib/tauri";
import { dismissRepoDocFix } from "../../../lib/tauri";
import { RepoDocsSection } from "./RepoDocsSection";

function fix(path: string, n: number, state: RepoDocFix["state"] = "in_pr"): RepoDocFix {
  return {
    id: `fix_${path}_${n}`,
    path,
    old_text: `Requires Node 1${n}`,
    new_text: `Requires Node 2${n}`,
    reason: "package.json now requires Node 20.",
    code: [{ path: "package.json", line_start: 5, line_end: 5 }],
    state,
    found_at: 1_792_660_000,
  };
}

const base: RepoWikiBinding = {
  space: "site",
  repo_root: "/Users/me/Repos/wenlan-site",
  folder: "wiki",
};

function renderSection(binding: RepoWikiBinding, onChanged = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <RepoDocsSection binding={binding} onChanged={onChanged} />
    </QueryClientProvider>,
  );
  return onChanged;
}

describe("RepoDocsSection", () => {
  beforeEach(() => vi.clearAllMocks());

  it("says when the docs are checked before any check ran", () => {
    renderSection(base);
    expect(screen.getByText("AGENTS.md")).toBeInTheDocument();
    expect(screen.queryByText("Matches the code")).not.toBeInTheDocument();
    expect(
      screen.getByText("Checked daily, or when you run /wenlan wiki."),
    ).toBeInTheDocument();
  });

  it("shows one line per doc, folds the rest, and opens a doc to its fixes", () => {
    renderSection({
      ...base,
      doc_check: { at: 1_792_660_000, docs: 14 },
      doc_fixes: [
        fix("README.md", 1),
        fix("README.md", 2, "pending"),
        fix("docs/a.md", 1),
        fix("docs/b.md", 1),
        fix("docs/c.md", 1),
        fix("docs/d.md", 1),
        fix("AGENTS.md", 9),
        fix("docs/e.md", 1, "dismissed"),
      ],
    });
    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(screen.getByText("2 fixes")).toBeInTheDocument();
    expect(screen.getByText("Next update")).toBeInTheDocument();
    expect(screen.queryByText("docs/c.md")).not.toBeInTheDocument();
    expect(screen.queryByText("docs/e.md")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more docs" }));
    expect(screen.getByText("docs/d.md")).toBeInTheDocument();
    expect(screen.getByText(/Checked 14 docs on .+\. The rest match the code\./)).toBeInTheDocument();

    expect(screen.queryByText("Requires Node 21")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /README\.md/ }));
    expect(screen.getByText("Requires Node 21")).toBeInTheDocument();
    expect(screen.getByText("Requires Node 22")).toBeInTheDocument();
  });

  it("opens AGENTS.md to its fixes and folds a long list of fixes", () => {
    renderSection({
      ...base,
      doc_fixes: [1, 2, 3, 4, 5].map((n) => fix("AGENTS.md", n)),
    });
    expect(screen.queryByText("Requires Node 21")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /AGENTS\.md/ }));
    expect(screen.getByText("Requires Node 23")).toBeInTheDocument();
    expect(screen.queryByText("Requires Node 24")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more fixes" }));
    expect(screen.getByText("Requires Node 25")).toBeInTheDocument();
  });

  it("drops a fix the user does not want", async () => {
    vi.mocked(dismissRepoDocFix).mockResolvedValue({ open: 0 });
    const onChanged = renderSection({ ...base, doc_fixes: [fix("README.md", 1)] });
    fireEvent.click(screen.getByRole("button", { name: /README\.md/ }));
    fireEvent.click(screen.getByRole("button", { name: "Don't change this" }));
    await waitFor(() => expect(dismissRepoDocFix).toHaveBeenCalledWith("site", "fix_README.md_1"));
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("says when a wiki page edited on GitHub was taken back or kept", () => {
    renderSection({
      ...base,
      last_publish: {
        status: "pr_updated",
        pages: 3,
        left_out: 0,
        waiting: 0,
        taken_back: 2,
        kept: 1,
        at: 1_792_660_000,
      },
    });
    expect(screen.getByText(/2 pages brought back from GitHub/)).toBeInTheDocument();
    expect(screen.getByText(/1 page waits for your review/)).toBeInTheDocument();
  });
});
