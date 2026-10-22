// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { open as openExternal } from "@tauri-apps/plugin-shell";

vi.mock("../../../lib/tauri", () => ({
  bindRepoWiki: vi.fn(),
  getRepoWiki: vi.fn(),
  publishRepoWiki: vi.fn(),
  suggestRepoWiki: vi.fn(),
  unbindRepoWiki: vi.fn(),
}));

import type { RepoWikiBinding } from "../../../lib/tauri";
import {
  bindRepoWiki,
  getRepoWiki,
  publishRepoWiki,
  suggestRepoWiki,
  unbindRepoWiki,
} from "../../../lib/tauri";
import { RepoWikiButton, githubRepo, prNumber } from "./RepoWikiButton";

const ROOT = "/Users/me/Repos/wenlan-site";

const bound: RepoWikiBinding = {
  space: "site",
  repo_root: ROOT,
  folder: "wiki",
  remote_url: "git@github.com:7xuanlu/wenlan-site.git",
  last_publish: {
    status: "pr_opened",
    pages: 12,
    left_out: 3,
    waiting: 2,
    branch: "wenlan/wiki",
    pr_url: "https://github.com/7xuanlu/wenlan-site/pull/261",
    at: 1_792_660_000,
  },
};

function renderButton() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><RepoWikiButton space="site" /></QueryClientProvider>);
}

describe("RepoWikiButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getRepoWiki).mockResolvedValue(null);
    vi.mocked(suggestRepoWiki).mockResolvedValue({ candidates: [] });
    vi.mocked(openExternal).mockResolvedValue(undefined);
  });

  it("reads GitHub remotes and pull request numbers", () => {
    expect(githubRepo("git@github.com:7xuanlu/wenlan-site.git")).toBe("github.com/7xuanlu/wenlan-site");
    expect(githubRepo("https://github.com/wenlan-app/wenlan")).toBe("github.com/wenlan-app/wenlan");
    expect(githubRepo("https://gitlab.com/a/b.git")).toBeNull();
    expect(prNumber("https://github.com/a/b/pull/261")).toBe("261");
    expect(prNumber(null)).toBeNull();
  });

  it("stays hidden on a Space with no code pages and no repository", async () => {
    renderButton();
    await waitFor(() => expect(suggestRepoWiki).toHaveBeenCalledWith("site"));
    expect(screen.queryByRole("button", { name: "Connect GitHub" })).not.toBeInTheDocument();
  });

  it("connects the suggested repository, then puts the pages in", async () => {
    vi.mocked(suggestRepoWiki).mockResolvedValue({
      candidates: [{ repo_root: ROOT, pages: 12, remote_url: bound.remote_url }],
    });
    vi.mocked(bindRepoWiki).mockResolvedValue({ ...bound, last_publish: null });
    vi.mocked(publishRepoWiki).mockResolvedValue(bound.last_publish!);
    renderButton();

    fireEvent.click(await screen.findByRole("button", { name: "Connect GitHub" }));
    expect(screen.getByLabelText("Repository folder on this computer")).toHaveValue(ROOT);
    expect(screen.getByText("Found github.com/7xuanlu/wenlan-site")).toBeInTheDocument();
    expect(screen.getByLabelText("Pages go in")).toHaveValue("wiki");
    fireEvent.click(screen.getByRole("button", { name: "Update every day" }));
    expect(screen.getByText(/Claude Code updates the pages in the background/)).toBeInTheDocument();
    expect(bindRepoWiki).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Update every day" }));
    vi.mocked(getRepoWiki).mockResolvedValue(bound);
    fireEvent.click(screen.getByRole("button", { name: "Connect and open PR" }));

    await waitFor(() => expect(publishRepoWiki).toHaveBeenCalledWith("site"));
    expect(bindRepoWiki).toHaveBeenCalledWith("site", ROOT, "wiki", { enabled: false, hour: 9, agent: "claude" });
    expect(await screen.findByRole("button", { name: "GitHub repository settings" })).toHaveTextContent("PR #261");
    expect(screen.getByText(/opened a PR with 12 pages/)).toBeInTheDocument();
  });

  it("shows the open PR, what stayed out, and opens the PR in the browser", async () => {
    vi.mocked(getRepoWiki).mockResolvedValue(bound);
    renderButton();

    const trigger = await screen.findByRole("button", { name: "GitHub repository settings" });
    expect(trigger).toHaveTextContent("PR #261");
    expect(suggestRepoWiki).not.toHaveBeenCalled();
    fireEvent.click(trigger);
    expect(screen.getByText("github.com/7xuanlu/wenlan-site")).toBeInTheDocument();
    expect(screen.getByText(/2 pages use code not on GitHub yet/)).toBeInTheDocument();
    expect(screen.getByText(/3 pages cite your notes or chats/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Open PR #261" }));
    expect(openExternal).toHaveBeenCalledWith("https://github.com/7xuanlu/wenlan-site/pull/261");
  });

  it("turns the daily update on with the hour and tool, keeping the folder", async () => {
    vi.mocked(getRepoWiki).mockResolvedValue(bound);
    vi.mocked(bindRepoWiki).mockImplementation(async (_space, _root, _folder, daily) => ({ ...bound, daily }));
    renderButton();

    fireEvent.click(await screen.findByRole("button", { name: "GitHub repository settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Update every day" }));
    await waitFor(() =>
      expect(bindRepoWiki).toHaveBeenCalledWith("site", ROOT, undefined, { enabled: true, hour: 9, agent: "claude" }));
    expect(await screen.findByText(/Claude Code updates the pages in the background/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Tool"), { target: { value: "codex" } });
    await waitFor(() =>
      expect(bindRepoWiki).toHaveBeenLastCalledWith("site", ROOT, undefined, { enabled: true, hour: 9, agent: "codex" }));
  });

  it("asks before disconnecting", async () => {
    vi.mocked(getRepoWiki).mockResolvedValue(bound);
    vi.mocked(unbindRepoWiki).mockResolvedValue(undefined);
    renderButton();

    fireEvent.click(await screen.findByRole("button", { name: "GitHub repository settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(unbindRepoWiki).not.toHaveBeenCalled();
    expect(screen.getByText(/Pages already merged stay in the repository/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(unbindRepoWiki).toHaveBeenCalledWith("site"));
  });
});
