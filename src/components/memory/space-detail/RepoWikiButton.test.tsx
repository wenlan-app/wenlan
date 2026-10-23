// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { open as openExternal } from "@tauri-apps/plugin-shell";

vi.mock("../../../lib/tauri", () => ({
  bindRepoWiki: vi.fn(),
  dismissRepoDocFix: vi.fn(),
  getRepoWiki: vi.fn(),
  getRepoWikiGithub: vi.fn(),
  publishRepoWiki: vi.fn(),
  setupRepoWikiGithub: vi.fn(),
  suggestRepoWiki: vi.fn(),
  unbindRepoWiki: vi.fn(),
}));

import type { RepoWikiBinding } from "../../../lib/tauri";
import {
  bindRepoWiki,
  getRepoWiki,
  getRepoWikiGithub,
  publishRepoWiki,
  setupRepoWikiGithub,
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
    expect(bindRepoWiki).toHaveBeenCalledWith("site", ROOT, "wiki", { enabled: false, hour: 9, agent: "claude", runs_on: "computer" });
    expect(await screen.findByRole("button", { name: "GitHub repository settings" })).toHaveTextContent("PR #261");
    expect(screen.getByText(/new PR with 12 pages/)).toBeInTheDocument();
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
      expect(bindRepoWiki).toHaveBeenCalledWith("site", ROOT, undefined, { enabled: true, hour: 9, agent: "claude", runs_on: "computer" }));
    expect(await screen.findByText(/Claude Code updates the pages in the background/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Tool"), { target: { value: "codex" } });
    await waitFor(() =>
      expect(bindRepoWiki).toHaveBeenLastCalledWith("site", ROOT, undefined, { enabled: true, hour: 9, agent: "codex", runs_on: "computer" }));
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

  it("runs the daily update on GitHub once its three steps are done", async () => {
    vi.mocked(getRepoWiki).mockResolvedValue({
      ...bound,
      daily: { enabled: true, hour: 9, agent: "claude", runs_on: "github" },
    });
    vi.mocked(getRepoWikiGithub).mockResolvedValue({
      repo: "7xuanlu/wenlan-site",
      gh_ready: true,
      workflow: "in_repo",
      secret: false,
      can_open_prs: false,
      last_run: {
        at: "2026-10-23T01:17:00Z",
        url: "https://github.com/7xuanlu/wenlan-site/actions/runs/1",
        status: "completed",
        conclusion: "success",
        pr_url: "https://github.com/7xuanlu/wenlan-site/pull/270",
      },
    });
    vi.mocked(setupRepoWikiGithub).mockImplementation(async (_space, request) => ({
      repo: "7xuanlu/wenlan-site",
      gh_ready: true,
      workflow: "in_repo",
      secret: request.action === "claude_token",
      can_open_prs: request.action === "allow_pull_requests",
    }));
    renderButton();
    fireEvent.click(await screen.findByRole("button", { name: "GitHub repository settings" }));

    expect(await screen.findByText("Runs with your computer off.")).toBeInTheDocument();
    expect((await screen.findByText("Add the daily job")).closest("[data-done]")).not.toBeNull();
    expect(screen.getByText("PR #270")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Set up" }));
    expect(screen.getByText("Wenlan never keeps or shows the key.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open browser" }));
    await waitFor(() => expect(setupRepoWikiGithub).toHaveBeenCalledWith("site", { action: "claude_token" }));
    await waitFor(() =>
      expect(screen.getByText("Let GitHub use your AI tool").closest("[data-done]")).not.toBeNull(),
    );
    expect(screen.queryByText("Wenlan never keeps or shows the key.")).toBeNull();
  });

  it("asks where the daily update runs", async () => {
    vi.mocked(getRepoWiki).mockResolvedValue({
      ...bound,
      daily: { enabled: true, hour: 9, agent: "codex" },
    });
    vi.mocked(bindRepoWiki).mockImplementation(async (_space, _root, _folder, daily) => ({
      ...bound,
      daily: daily ?? null,
    }));
    vi.mocked(getRepoWikiGithub).mockResolvedValue({
      repo: "7xuanlu/wenlan-site",
      gh_ready: true,
      workflow: "missing",
      secret: false,
      can_open_prs: true,
    });
    renderButton();
    fireEvent.click(await screen.findByRole("button", { name: "GitHub repository settings" }));
    fireEvent.click(await screen.findByRole("button", { name: "GitHub" }));
    await waitFor(() =>
      expect(bindRepoWiki).toHaveBeenCalledWith("site", ROOT, undefined, {
        enabled: true,
        hour: 9,
        agent: "codex",
        runs_on: "github",
      }),
    );
    // The step names no tool; the box it opens is the chosen tool's.
    expect(await screen.findByText("Let GitHub use your AI tool")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Set up" }));
    expect(await screen.findByText("Use your OpenAI key", { selector: "span" })).toBeInTheDocument();
    expect(screen.getByText("Allow it to open PRs").closest("[data-done]")).not.toBeNull();
  });
});
