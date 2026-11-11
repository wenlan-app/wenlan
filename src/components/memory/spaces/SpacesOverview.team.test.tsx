// SPDX-License-Identifier: AGPL-3.0-only
import { screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { makeSpace, renderOverview } from "./SpacesOverview.testUtils";

const api = vi.hoisted(() => ({ listSpaces: vi.fn(), listPages: vi.fn(), listShares: vi.fn() }));
vi.mock("../../../lib/tauri", () => api);

const share = (target: string, extra: object) => ({
  kind: "space", target, folder: "", member_id: "me", member_name: "Me", members: [], ...extra,
});

beforeEach(async () => {
  await i18n.changeLanguage("en");
  api.listPages.mockResolvedValue([]);
  api.listSpaces.mockResolvedValue([
    makeSpace({ id: "a", name: "Reading", sort_order: 0 }),
    makeSpace({ id: "b", name: "Product", sort_order: 1 }),
  ]);
});

describe("SpacesOverview team groups", () => {
  it("titles the person's own Spaces only when a team group exists", async () => {
    api.listShares.mockResolvedValue([share("Product", { host: "https://team.example.com" })]);
    renderOverview();
    expect(await screen.findByRole("heading", { name: "Team · team.example.com" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Mine" })).toBeInTheDocument();
  });

  it("shows no title when nothing is shared with a team", async () => {
    api.listShares.mockResolvedValue([]);
    renderOverview();
    await screen.findByText("Reading");
    expect(screen.queryByRole("heading", { name: "Mine" })).not.toBeInTheDocument();
  });
});
