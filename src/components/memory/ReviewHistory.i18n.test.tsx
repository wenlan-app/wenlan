// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";

vi.mock("../../lib/tauri", () => ({
  getMemoryRevisions: vi.fn(),
  getPageRevisions: vi.fn(),
  listRecentChanges: vi.fn(async () => [
    {
      page_id: "page-1",
      title: "Page One",
      change_kind: "created",
      changed_at_ms: Date.now() - 9 * 60_000,
    },
  ]),
}));

import { RecentRevisionsSection } from "./ReviewHistory";

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <RecentRevisionsSection onPageClick={() => {}} />
    </QueryClientProvider>,
  );
}

afterEach(async () => {
  await i18n.changeLanguage("en");
});

describe("RecentRevisionsSection time", () => {
  it("keeps the short English relative time", async () => {
    await i18n.changeLanguage("en");
    renderSection();
    expect(await screen.findByText("9m ago")).toBeInTheDocument();
  });

  it("writes the relative time in Traditional Chinese, not English", async () => {
    await i18n.changeLanguage("zh-Hant");
    renderSection();
    await screen.findByText("Page One");
    expect(screen.queryByText(/ago/)).not.toBeInTheDocument();
    expect(screen.getByText(/9\s*分鐘前/)).toBeInTheDocument();
  });
});
