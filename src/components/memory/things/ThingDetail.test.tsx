// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  memories: [
    { source_id: "mem_2", title: "The hero line stays under eight words", memory_type: "preference", created_at: now - 300 },
    { source_id: "mem_1", title: "Lead with the X wording.", memory_type: "decision", created_at: now - 900 },
  ],
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
    localStorage.clear();
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
    // The tool shows by its mark, as on Home; a page says the AI wrote it.
    expect(screen.getByRole("img", { name: "Claude Code" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Tagline options/ })).toHaveTextContent("AI wrote");
    fireEvent.click(screen.getByRole("button", { name: /^Tagline options/ }));
    expect(onOpenPage).toHaveBeenCalledWith("page-1");
  });

  it("UX-VIEW-1: short text and mind map show the same points, and the last view is remembered", async () => {
    const { onOpenPage } = renderThing();
    const short = await screen.findByTestId("thing-short");
    // Points read in the order they were worked out.
    expect(short).toHaveTextContent("Lead with the X wording. The hero line stays under eight words.");
    fireEvent.click(screen.getByRole("button", { name: "Mind map" }));
    const map = screen.getByTestId("thing-map");
    expect(map).toHaveTextContent("Lead with the X wording.");
    expect(screen.queryByTestId("thing-short")).toBeNull();
    fireEvent.click(within(map).getByRole("button", { name: "Tagline options" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-1");
    cleanup();
    renderThing();
    expect(await screen.findByTestId("thing-map")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Mind map" })).toHaveAttribute("aria-pressed", "true");
  });

  it("UX-THING-1: renames the thing in one step", async () => {
    vi.mocked(renameThing).mockResolvedValue({ ...tagline, title: "Tagline", title_by: "person" });
    renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const input = screen.getByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: "Tagline" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(renameThing).toHaveBeenCalledWith("thg_1", "Tagline"));
  });

  it("UX-THING-1: moves a conversation to another thing in one step", async () => {
    vi.mocked(getThing).mockResolvedValue({
      ...tagline,
      sessions: [...tagline.sessions, { id: "ses_b", tool: "codex", title: "Shorter variants", last_write_at: now - 900 }],
    });
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_2", title: "Product Hunt launch" });
    const { onOpenThing } = renderThing();
    fireEvent.click((await screen.findAllByRole("button", { name: "Move" }))[0]);
    fireEvent.click(await screen.findByRole("button", { name: "Product Hunt launch" }));
    await waitFor(() => expect(moveSessionThing).toHaveBeenCalledWith("ses_a", "thg_1", { thingId: "thg_2" }));
    fireEvent.click(await screen.findByRole("button", { name: "Open" }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_2");
  });

  it("UX-CHAT-5: moving the last conversation out goes along with it", async () => {
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_2", title: "Product Hunt launch" });
    const { onOpenThing } = renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Move" }));
    fireEvent.click(await screen.findByRole("button", { name: "Product Hunt launch" }));
    await waitFor(() => expect(onOpenThing).toHaveBeenCalledWith("thg_2"));
  });

  it("UX-THING-1: moves a conversation into a new thing it names", async () => {
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_3", title: "Hero copy" });
    renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Move" }));
    fireEvent.change(screen.getByRole("textbox", { name: "New name" }), { target: { value: "Hero copy" } });
    fireEvent.click(screen.getByRole("button", { name: "Start new" }));
    await waitFor(() => expect(moveSessionThing).toHaveBeenCalledWith("ses_a", "thg_1", { newTitle: "Hero copy" }));
  });
});
