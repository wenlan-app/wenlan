// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { LookData, Page, PageLook } from "../../../lib/tauri";

const mocks = vi.hoisted(() => ({
  getPageLook: vi.fn(),
  getPageLookData: vi.fn(),
  removePageLook: vi.fn(),
  restorePageLook: vi.fn(),
  reportPageLookError: vi.fn(),
  clipboardWrite: vi.fn(),
}));

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  ...mocks,
}));

import { HOME_LOOK_ID, HomeLookView, homeLookVersion, useHomeLook } from "./HomeLook";
import HomeLookSetting from "../settings/sections/HomeLookSetting";

const LOOK: PageLook = { id: 3, html: "<main id=home></main>", api: 1, kept_at: 1 };
const DATA: LookData = {
  api: 1,
  page: { id: "home", title: "", space: "", created: "", modified: "", text: "", html: "", sections: [], links: [] },
  space: { name: "", pages: [], more: false },
  theme: { mode: "dark" },
  home: {
    spaces: [
      { name: "Travel", description: "", starred: true, modified: "2026-11-02" },
      { name: "Acme roadmap", description: "", starred: false, modified: "2026-11-03" },
    ],
    pages: [
      { id: "pg_team", title: "Acme launch plan", space: "Acme roadmap", created: "", modified: "2026-11-03", summary: "" },
      { id: "pg_trip", title: "Japan trip", space: "Travel", created: "", modified: "2026-11-02", summary: "" },
      { id: "pg_loose", title: "Loose idea", space: "", created: "", modified: "2026-11-01", summary: "" },
    ],
    more: false,
  },
};

function Home({ supported = true, onOpenPage = () => {} }: { supported?: boolean; onOpenPage?: (id: string) => void }) {
  const homeLook = useHomeLook({ supported, pages: [] });
  return (
    <div className="home-columns">
      <HomeLookView homeLook={homeLook} inScope={(space) => space !== "Acme roadmap"} onOpenPage={onOpenPage} />
      {homeLook.shown ? null : <p>usual home</p>}
    </div>
  );
}

function renderHome(props: Parameters<typeof Home>[0] = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><Home {...props} /></QueryClientProvider>);
}

function renderSetting() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><HomeLookSetting /></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
  mocks.getPageLookData.mockResolvedValue(DATA);
  mocks.removePageLook.mockResolvedValue(3);
  mocks.restorePageLook.mockResolvedValue(undefined);
  mocks.reportPageLookError.mockResolvedValue(undefined);
  mocks.clipboardWrite.mockResolvedValue(undefined);
});

describe("UX-LOOK-10 home's HTML view", () => {
  it("opens on the view, filled with this side's pages and Spaces only", async () => {
    renderHome();
    const frame = await screen.findByTestId("page-look-frame");
    expect(mocks.getPageLook).toHaveBeenCalledWith(HOME_LOOK_ID);
    expect(mocks.getPageLookData).toHaveBeenCalledWith(HOME_LOOK_ID);
    const doc = frame.getAttribute("srcdoc") ?? "";
    expect(doc).toContain("Japan trip");
    expect(doc).toContain("Loose idea");
    expect(doc).not.toContain("Acme launch plan");
    expect(doc).not.toContain("Acme roadmap");
    expect(screen.queryByText("usual home")).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("Settings › Appearance › Home switches to the usual home, and home remembers it", async () => {
    const setting = renderSetting();
    fireEvent.click(await screen.findByRole("button", { name: "Usual" }));
    setting.unmount();

    renderHome();
    await waitFor(() => expect(mocks.getPageLook).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText("usual home")).toBeTruthy());
    expect(screen.queryByTestId("page-look-frame")).toBeNull();
  });

  it("a new view from an AI opens on home again even after the usual home was chosen", async () => {
    const setting = renderSetting();
    fireEvent.click(await screen.findByRole("button", { name: "Usual" }));
    setting.unmount();

    mocks.getPageLook.mockResolvedValue({ look: { ...LOOK, id: 4 }, versions: [] });
    renderHome();
    expect(await screen.findByTestId("page-look-frame")).toBeTruthy();
  });

  it("Settings takes the view off with Undo, and without one says how to ask for it", async () => {
    renderSetting();
    fireEvent.click(await screen.findByRole("button", { name: "Remove HTML view" }));
    mocks.getPageLook.mockResolvedValue({ look: null, versions: [] });
    await waitFor(() => expect(mocks.removePageLook).toHaveBeenCalledWith(HOME_LOOK_ID));
    expect(await screen.findByText("Ask your AI to make your home an HTML view.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Copy request" }));
    await waitFor(() => expect(mocks.clipboardWrite).toHaveBeenCalledWith(expect.stringContaining("@home")));
  });

  it("shows nothing new when home has no view, or on a team's home", async () => {
    mocks.getPageLook.mockResolvedValue({ look: null, versions: [] });
    renderHome();
    await waitFor(() => expect(mocks.getPageLook).toHaveBeenCalled());
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.getByText("usual home")).toBeTruthy();

    mocks.getPageLook.mockClear();
    mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    renderHome({ supported: false });
    expect(mocks.getPageLook).not.toHaveBeenCalled();
  });

  it("reads again whenever a page changes", () => {
    const page = (modified: string) => ({ last_modified: modified, created_at: "" }) as Page;
    expect(homeLookVersion([page("2026-11-01")])).not.toBe(homeLookVersion([page("2026-11-02")]));
    expect(homeLookVersion([page("2026-11-01")])).not.toBe(homeLookVersion([page("2026-11-01"), page("2026-10-01")]));
  });
});
