// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { makeSpace, renderOverview } from "./SpacesOverview.testUtils";

const api = vi.hoisted(() => ({ listSpaces: vi.fn(), listPages: vi.fn(), listShares: vi.fn(), teamJoinedLibrary: vi.fn() }));
vi.mock("../../../lib/tauri", () => api);

const share = (target: string, extra: object) => ({
  kind: "space", target, folder: "", member_id: "me", member_name: "Me", members: [], ...extra,
});

beforeEach(async () => {
  await i18n.changeLanguage("en");
  api.listPages.mockResolvedValue([]);
  api.teamJoinedLibrary.mockResolvedValue({ teams: [] });
  api.listSpaces.mockResolvedValue([
    makeSpace({ id: "a", name: "Reading", sort_order: 0 }),
    makeSpace({ id: "b", name: "Product", sort_order: 1 }),
  ]);
});

describe("SpacesOverview team groups", () => {
  it("titles the person's own Spaces only when a team group exists", async () => {
    api.listShares.mockResolvedValue([share("Product", { host: "https://team.example.com", team: { id: "t1", name: "Acme" } })]);
    renderOverview();
    expect(await screen.findByRole("heading", { name: "Acme" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Private" })).toBeInTheDocument();
  });

  it("shows no title when nothing is shared with a team", async () => {
    api.listShares.mockResolvedValue([]);
    renderOverview();
    await screen.findByText("Reading");
    expect(screen.queryByRole("heading", { name: "Private" })).not.toBeInTheDocument();
  });

  it("UX-TEAM-MEMBER-1: renders live team Spaces and published pages without Space actions", async () => {
    const open = vi.fn();
    api.listShares.mockResolvedValue([share("Product", { host: "https://team.example.com", team: { id: "t1", name: "Acme" } })]);
    api.teamJoinedLibrary.mockResolvedValue({
      teams: [{
        team: { id: "t1", name: "Acme" },
        library: {
          state: "ready",
          spaces: [{
            space: { id: "library-roadmap", name: "Roadmap", restricted: false },
            pages: [{ id: "page-1", title: "Launch plan", space: "Roadmap", published_by: null, published_at: null }],
          }],
        },
      }],
    });
    renderOverview({ onOpenTeamPage: open });
    const group = await screen.findByTestId("team-joined-library");
    expect(within(group).getByRole("heading", { name: "Team · Acme" })).toBeInTheDocument();
    expect(within(group).getByRole("heading", { name: "Roadmap" })).toBeInTheDocument();
    fireEvent.click(within(group).getByRole("button", { name: "Launch plan" }));
    expect(open).toHaveBeenCalledWith("t1", "page-1");
    expect(within(group).queryByRole("button", { name: /Actions for|Rename|Delete|New/ })).not.toBeInTheDocument();
  });
});
