// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../../lib/tauri")>()),
  listRecentChanges: vi.fn(),
  getPageRevisions: vi.fn(),
  updatePage: vi.fn(),
}));

import { getPageRevisions, listRecentChanges, updatePage, type SharedStatus } from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import { SpaceRecentChanges, changeWho } from "./SpaceRecentChanges";

const shares = [{
  kind: "space", target: "Product", folder: "", host: "https://team.example.com", read_only: false, hosted: false,
  member_id: "me", member_name: "Lucian",
  members: [{ id: "me", name: "Lucian" }, { id: "amy", name: "Amy" }, { id: "bo", name: "Bo" }],
}] as unknown as SharedStatus[];

function renderChanges() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SpaceRecentChanges space="Product" shares={shares} /></QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.mocked(listRecentChanges).mockReset().mockResolvedValue([
    { page_id: "p1", title: "Roadmap", change_kind: "revised", changed_at_ms: Date.now() - 60_000, edited_by: "shared_person:amy", changed_sentences: 3 },
    { page_id: "p2", title: "Pricing", change_kind: "created", changed_at_ms: Date.now() - 3_600_000, edited_by: "shared_ai:bo" },
    { page_id: "p3", title: "Notes", change_kind: "revised", changed_at_ms: Date.now() - 7_200_000, edited_by: "manual_edit", changed_sentences: 1 },
  ]);
  vi.mocked(getPageRevisions).mockReset().mockResolvedValue({
    page_id: "p1", current_version: 4, user_edited: false, entries: [],
    history: [
      { kind: "person_edit", at: 1_700_000_100, version: 4, edited_by: "shared_person:amy", text: "Ship in May. Hire two." },
      { kind: "person_edit", at: 1_700_000_000, version: 3, edited_by: "manual_edit", text: "Ship in June." },
    ],
  });
  vi.mocked(updatePage).mockReset().mockResolvedValue({ outcome: "saved" });
});

describe("recent changes on a shared Space", () => {
  it("names who changed which page and how many sentences, newest first", async () => {
    renderChanges();
    expect(await screen.findByText("Amy changed Roadmap")).toBeInTheDocument();
    expect(screen.getByText("Bo's AI tool added Pricing")).toBeInTheDocument();
    expect(screen.getByText("You changed Notes")).toBeInTheDocument();
    expect(screen.getByText(/^3 sentences · /)).toBeInTheDocument();
    expect(screen.getByText(/^New page · /)).toBeInTheDocument();
    expect(listRecentChanges).toHaveBeenCalledWith(5, "Product");
  });

  it("opens the change and puts the earlier text back as this person's edit", async () => {
    renderChanges();
    fireEvent.click(await screen.findByText("Amy changed Roadmap"));
    fireEvent.click(await screen.findByRole("button", { name: "Put back the earlier text" }));
    await waitFor(() => expect(updatePage).toHaveBeenCalledWith(expect.objectContaining({
      id: "p1", content: "Ship in June.", expectedVersion: 4, callerId: "wenlan-app",
    })));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("says so when the page changed again before the text could go back", async () => {
    vi.mocked(updatePage).mockResolvedValue({ outcome: "conflict", message: "version moved" });
    renderChanges();
    fireEvent.click(await screen.findByText("Amy changed Roadmap"));
    fireEvent.click(await screen.findByRole("button", { name: "Put back the earlier text" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't put it back");
  });

  it("names a page the person started in the app and published, or restored, as theirs, never their AI tool", () => {
    const t = i18n.getFixedT("en");
    expect(changeWho("publish", [], t)).toBe("You");
    expect(changeWho("restore", [], t)).toBe("You");
    expect(changeWho("agent_create", [], t)).toBe("Your AI tool");
  });

  it("shows nothing when the Space has no changes yet", async () => {
    vi.mocked(listRecentChanges).mockResolvedValue([]);
    const { container } = renderChanges();
    await waitFor(() => expect(listRecentChanges).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});
