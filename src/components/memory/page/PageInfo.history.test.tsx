// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import type { PageChangelogEntry, PageHistoryItem } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getPageRevisions: vi.fn(() => new Promise(() => {})),
}));

import PageInfo from "./PageInfo";

const entry = (version: number): PageChangelogEntry => ({ version, at: 100 + version, edited_by: "manual_edit" });

// Versions 2–4 are one burst of saves; the daemon lists them once.
const history: PageHistoryItem[] = [
  { kind: "person_edit", at: 104, version: 4, first_version: 2, edited_by: "manual_edit" },
  { kind: "ai_edit", at: 101, version: 1, edited_by: "create" },
];

function setup(openVersion?: number) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onOpenedVersion = vi.fn();
  render(<QueryClientProvider client={client}><PageInfo
    sourceCount={0}
    sources={[]}
    inbound={[]}
    revisions={[entry(1), entry(2), entry(3), entry(4)]}
    history={history}
    citations={undefined}
    citationState="none"
    onMemoryClick={vi.fn()}
    currentPageId="page-1"
    openVersion={openVersion}
    onOpenedVersion={onOpenedVersion}
  /></QueryClientProvider>);
  return { onOpenedVersion };
}

afterEach(cleanup);

describe("PageInfo history", () => {
  it("UX-HIST-3: a burst of saves counts as one revision", () => {
    setup();
    expect(screen.getByText(/2 revisions/)).toBeInTheDocument();
  });

  it("UX-HIST-3: a version inside a folded run still opens its change", () => {
    const { onOpenedVersion } = setup(3);
    expect(onOpenedVersion).toHaveBeenCalled();
  });
});
