// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getThing, listThings, moveSessionThing, renameThing, type ThingDetail as ThingRecord } from "../../../lib/tauri";
import { ThingDetail } from "./ThingDetail";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getThing: vi.fn(),
  listThings: vi.fn(),
  renameThing: vi.fn(),
  moveSessionThing: vi.fn(),
}));

const now = Math.floor(Date.now() / 1000);

const tagline: ThingRecord = {
  id: "thg_1",
  title: "New tagline",
  title_by: "ai",
  status_line: "Picked the X wording; the site still shows the old line.",
  spaces: ["wenlan"],
  last_activity_at: now - 600,
  sessions: [{ id: "ses_a", tool: "claude-code", title: "Rewrite the hero line", last_write_at: now - 600 }],
  pages: [{ id: "page-1", title: "Tagline options", via: "wrote", at: now - 600 }],
};

function renderThing(onOpenPage = vi.fn(), onOpenThing = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <ThingDetail thingId="thg_1" onBack={vi.fn()} onOpenPage={onOpenPage} onOpenThing={onOpenThing} />
    </QueryClientProvider>,
  );
  return { onOpenPage, onOpenThing };
}

describe("ThingDetail", () => {
  beforeEach(() => {
    vi.mocked(getThing).mockReset().mockResolvedValue(tagline);
    vi.mocked(listThings).mockReset().mockResolvedValue([
      tagline,
      { ...tagline, id: "thg_2", title: "Product Hunt launch" },
    ]);
    vi.mocked(renameThing).mockReset();
    vi.mocked(moveSessionThing).mockReset();
  });

  it("UX-CHAT-5: shows where the thing stands, its conversations and pages, with no done state", async () => {
    const { onOpenPage } = renderThing();
    expect(await screen.findByRole("heading", { level: 1, name: "New tagline" })).toBeTruthy();
    expect(screen.getByText(/Picked the X wording/)).toBeTruthy();
    expect(screen.getByText("Rewrite the hero line")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /done|wrap up|archive/i })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Tagline options" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-1");
  });

  it("UX-THING-1: renames the thing in one step", async () => {
    vi.mocked(renameThing).mockResolvedValue({ ...tagline, title: "Tagline", title_by: "person" });
    renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const input = screen.getByRole("textbox", { name: "Name of this thing" });
    fireEvent.change(input, { target: { value: "Tagline" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(renameThing).toHaveBeenCalledWith("thg_1", "Tagline"));
  });

  it("UX-THING-1: moves a conversation to another thing in one step", async () => {
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_2", title: "Product Hunt launch" });
    const { onOpenThing } = renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Move" }));
    fireEvent.click(await screen.findByRole("button", { name: "Product Hunt launch" }));
    await waitFor(() => expect(moveSessionThing).toHaveBeenCalledWith("ses_a", "thg_1", { thingId: "thg_2" }));
    fireEvent.click(await screen.findByRole("button", { name: "Open" }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_2");
  });

  it("UX-THING-1: moves a conversation into a new thing it names", async () => {
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_3", title: "Hero copy" });
    renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Move" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Name of the new thing" }), { target: { value: "Hero copy" } });
    fireEvent.click(screen.getByRole("button", { name: "New thing" }));
    await waitFor(() => expect(moveSessionThing).toHaveBeenCalledWith("ses_a", "thg_1", { newTitle: "Hero copy" }));
  });
});
