// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { listSetAsideMaterial, releaseSetAsideMaterial, type SetAsideGroup } from "../../../lib/tauri";
import ActivitySetAside from "./ActivitySetAside";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listSetAsideMaterial: vi.fn(),
  releaseSetAsideMaterial: vi.fn(),
  setUiLanguage: vi.fn(() => Promise.resolve()),
}));

const now = Math.floor(Date.now() / 1000);

function group(name: string, ids: string[], reason = "too_short"): SetAsideGroup {
  return { name, reason, items: ids.map((id) => ({ item_id: id, title: `${name} ${id}`, kind: "note" })), set_aside_at: now - 60 };
}

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ActivitySetAside />
    </QueryClientProvider>,
  );
}

describe("ActivitySetAside", () => {
  beforeEach(async () => {
    vi.mocked(listSetAsideMaterial).mockReset().mockResolvedValue({ groups: [] });
    vi.mocked(releaseSetAsideMaterial).mockReset().mockResolvedValue({ released: 2 });
    await i18n.changeLanguage("en");
  });
  afterEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("UX-HOME-7 is absent when nothing is set aside", async () => {
    renderSection();
    await waitFor(() => expect(listSetAsideMaterial).toHaveBeenCalled());
    expect(screen.queryByTestId("activity-set-aside")).toBeNull();
  });

  it("UX-HOME-7 lists each set-aside group by name with why it was set aside", async () => {
    vi.mocked(listSetAsideMaterial).mockResolvedValue({
      groups: [group("Driftwood", ["a"]), group("Kyoto", ["b"], "sources_did_not_match"), group("Notes", ["c"], "something_new")],
    });
    renderSection();
    const rows = await screen.findAllByTestId("activity-set-aside-row");
    expect(screen.getByTestId("activity-set-aside")).toHaveTextContent("Not written yet");
    expect(rows.map((row) => row.querySelector("h4")?.textContent)).toEqual(["Driftwood", "Kyoto", "Notes"]);
    expect(rows[0]).toHaveTextContent("Too little to write a page from");
    expect(rows[1]).toHaveTextContent("The page did not match its sources");
    expect(rows[2]).toHaveTextContent("Writing stopped before the page was done");
  });

  it("UX-HOME-7 Try again gives back only that group's items and reads the list again", async () => {
    vi.mocked(listSetAsideMaterial)
      .mockResolvedValueOnce({ groups: [group("Driftwood", ["itm_1", "itm_2"]), group("Kyoto", ["itm_3"])] })
      .mockResolvedValue({ groups: [] });
    renderSection();
    const rows = await screen.findAllByTestId("activity-set-aside-row");
    fireEvent.click(within(rows[0]).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(releaseSetAsideMaterial).toHaveBeenCalledWith(["itm_1", "itm_2"]));
    await waitFor(() => expect(screen.queryByTestId("activity-set-aside")).toBeNull());
  });

  it("UX-HOME-7 speaks Traditional Chinese", async () => {
    await i18n.changeLanguage("zh-Hant");
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [group("設計的心理學", ["a"])] });
    renderSection();
    const row = await screen.findByTestId("activity-set-aside-row");
    expect(screen.getByTestId("activity-set-aside")).toHaveTextContent("還沒寫成頁");
    expect(row).toHaveTextContent("材料太少，寫不成一頁");
    expect(within(row).getByRole("button")).toHaveTextContent("再試一次");
  });
});
