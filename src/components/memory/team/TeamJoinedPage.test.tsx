// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { TeamJoinedPage } from "./TeamJoinedPage";
import { teamJoinedPage } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async () => ({
  ...(await vi.importActual<typeof import("../../../lib/tauri")>("../../../lib/tauri")),
  teamJoinedPage: vi.fn(),
}));

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return { client, ...render(<QueryClientProvider client={client}><TeamJoinedPage teamId="t1" pageId="p1" onBack={vi.fn()} /></QueryClientProvider>) };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.mocked(teamJoinedPage).mockReset();
});

describe("TeamJoinedPage", () => {
  it("UX-TEAM-MEMBER-2: shows the publisher line, read-only label and redacts private citation text", async () => {
    vi.mocked(teamJoinedPage).mockResolvedValue({
      state: "ready",
      page: { id: "p1", title: "Launch plan", body: "The plan is ready.", space: "Roadmap", published_by: { id: "person-1", name: "Qi-Xuan", owner: false }, published_at: "2026-11-26T12:00:00Z", read_only: true },
      sources: { page_id: "p1", sources: [
        { kind: "private_source", label: "private source" },
        { kind: "memory", memory: { team_id: "t1", item_id: "m1", kind: "memory", revision: 1, space_id: "s1", title: "Shared memory", body: "shared words", contributor: null, authorship: "human", publisher: null, last_editor: null, published_at: null } },
      ] },
    });
    renderPage();
    expect(await screen.findByRole("heading", { name: "Launch plan" })).toBeInTheDocument();
    expect(screen.getByText(/Qi-Xuan published/)).toBeInTheDocument();
    expect(screen.getByText(/read only/)).toBeInTheDocument();
    expect(screen.getByText("private source")).toBeInTheDocument();
    expect(screen.queryByText("private source" )?.textContent).toBe("private source");
    expect(screen.queryByText("secret words")).not.toBeInTheDocument();
  });

  it("UX-TEAM-MEMBER-3: uses the plain unreachable state and retry without exposing transport details", async () => {
    vi.mocked(teamJoinedPage).mockResolvedValue({ state: "unreachable", reason: "can't reach the team server: https://secret.example/api/t1" });
    renderPage();
    expect(await screen.findByText("Can't reach the team server right now.")).toBeInTheDocument();
    expect(screen.queryByText(/secret\.example|api\/t1|https/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(teamJoinedPage).toHaveBeenCalledTimes(2);
  });
});
