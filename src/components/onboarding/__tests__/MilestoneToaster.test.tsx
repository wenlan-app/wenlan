// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, it, expect, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import { toast } from "sonner";
import { MilestoneToaster } from "../MilestoneToaster";
import { AppToaster } from "../../AppToaster";
import { FirstPageModal } from "../FirstPageModal";
import type { MilestoneRecord } from "../../../lib/tauri";

const recentRecall: MilestoneRecord = {
  id: "first-recall",
  first_triggered_at: Math.floor(Date.now() / 1000) - 10,
  acknowledged_at: null,
  payload: { agent: "claude" },
};

const oldRecall: MilestoneRecord = {
  ...recentRecall,
  first_triggered_at: Math.floor(Date.now() / 1000) - 60 * 60 * 48, // 48h old
};

vi.mock("../../../lib/tauri", () => ({
  listOnboardingMilestones: vi.fn(),
  acknowledgeOnboardingMilestone: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const { listOnboardingMilestones } = await import("../../../lib/tauri");
const mockList = vi.mocked(listOnboardingMilestones);
const { acknowledgeOnboardingMilestone } = await import("../../../lib/tauri");
const mockAcknowledge = vi.mocked(acknowledgeOnboardingMilestone);

afterEach(() => {
  act(() => { toast.dismiss(); });
  vi.useRealTimers();
  Reflect.deleteProperty(document, "hidden");
  mockList.mockReset();
  mockAcknowledge.mockClear();
});

function renderWithSeededMilestones(records: MilestoneRecord[]) {
  mockList.mockResolvedValue(records);
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  queryClient.setQueryData(["onboarding-milestones"], records);
  const view = render(
    <QueryClientProvider client={queryClient}>
      <AppToaster />
      <MilestoneToaster />
    </QueryClientProvider>,
  );
  return { queryClient, view };
}

// Sonner marks a dismissed toast on an animation frame, so frames are faked too.
function useFakeTimers() {
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date", "requestAnimationFrame", "cancelAnimationFrame"],
  });
}

// Sonner adds a toast on a timer and removes it after a short exit animation.
const SONNER_EXIT_MS = 1_000;

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

const settle = () => advance(0);
async function settleExit() {
  // Marked removed on one turn, unmounted after its exit timer on the next.
  await advance(SONNER_EXIT_MS);
  await advance(SONNER_EXIT_MS);
}

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={qc}>
      <AppToaster />
      {children}
    </QueryClientProvider>
  );
}

describe("MilestoneToaster", () => {
  it("renders a toast for a recent unacknowledged toast-channel milestone", async () => {
    mockList.mockResolvedValueOnce([recentRecall]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(screen.getByText(/recalled for you/i)).toBeInTheDocument(),
    );
  });

  it("holds toasts back while the first-page dialog is open", async () => {
    mockList.mockResolvedValue([recentRecall]);
    const page = { id: "c1", title: "Trip plan", source_memory_ids: ["m1"] };
    const view = render(
      <>
        <MilestoneToaster />
        <FirstPageModal page={page} onOpen={vi.fn()} onDismiss={vi.fn()} />
      </>,
      { wrapper },
    );
    await waitFor(() => expect(mockList).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByText(/recalled for you/i)).toBeNull();
    view.rerender(<MilestoneToaster />);
    await waitFor(() => expect(screen.getByText(/recalled for you/i)).toBeInTheDocument());
  });

  it("does not render a toast for milestones older than 24h", async () => {
    mockList.mockResolvedValueOnce([oldRecall]);
    const { container } = render(<MilestoneToaster />, { wrapper });
    await waitFor(() => expect(mockList).toHaveBeenCalled());
    expect(container.textContent).not.toMatch(/recalled for you/i);
  });

  it("renders a toast for first-memory", async () => {
    const firstMemory: MilestoneRecord = {
      id: "first-memory",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: null,
    };
    mockList.mockResolvedValueOnce([firstMemory]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(screen.getByText(/first memory saved/i)).toBeInTheDocument(),
    );
  });

  it("renders quoted preview + source attribution for first-memory with payload", async () => {
    const firstMemory: MilestoneRecord = {
      id: "first-memory",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: {
        memory_id: "mem_abc",
        source: "claude",
        preview: "I prefer Rust for CLI tools because of compile-time safety.",
      },
    };
    mockList.mockResolvedValueOnce([firstMemory]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(
        screen.getByText(/I prefer Rust for CLI tools/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(/from claude/i)).toBeInTheDocument();
  });

  it("omits source attribution when first-memory source is empty", async () => {
    const firstMemory: MilestoneRecord = {
      id: "first-memory",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: {
        memory_id: "mem_abc",
        source: "", // empty string — should be treated as missing
        preview: "Fresh note from the daemon.",
      },
    };
    mockList.mockResolvedValueOnce([firstMemory]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(screen.getByText(/Fresh note from the daemon/i)).toBeInTheDocument(),
    );
    // No attribution line should appear. The preview itself says "from the
    // daemon", so the guard excludes that wording.
    expect(document.body.textContent).not.toMatch(/from (?!the daemon)/);
  });

  it("renders agent subtitle for second-agent", async () => {
    const secondAgent: MilestoneRecord = {
      id: "second-agent",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: { agent: "cursor" },
    };
    mockList.mockResolvedValueOnce([secondAgent]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(screen.getByText(/A second AI/i)).toBeInTheDocument(),
    );
    expect(
      screen.getByText(/cursor joined.*memories follow you across tools/i),
    ).toBeInTheDocument();
  });

  it("renders the full intelligence-ready milestone presentation", async () => {
    const ready: MilestoneRecord = {
      id: "intelligence-ready",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: null,
    };
    mockList.mockResolvedValueOnce([ready]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(screen.getByText("Intelligence ready", { exact: true })).toBeInTheDocument(),
    );
    expect(screen.getByText(/on-device intelligence/i)).toBeInTheDocument();
    expect(screen.getByText(/run locally/i)).toBeInTheDocument();
  });

  it("renders rephrased first-recall copy", async () => {
    const recall: MilestoneRecord = {
      id: "first-recall",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: { agent: "claude" },
    };
    mockList.mockResolvedValueOnce([recall]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(
        screen.getByText(/wenlan just recalled for you/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(/called by claude/i)).toBeInTheDocument();
  });

  it("quotes the recalled excerpt when first-recall payload has preview", async () => {
    const recall: MilestoneRecord = {
      id: "first-recall",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: {
        agent: "claude-code",
        preview: "Origin uses Cloudflare quick tunnels.",
      },
    };
    mockList.mockResolvedValueOnce([recall]);
    render(<MilestoneToaster />, { wrapper });
    await waitFor(() =>
      expect(screen.getByText(/Cloudflare quick tunnels/i)).toBeInTheDocument(),
    );
    expect(screen.getByText(/from claude-code/i)).toBeInTheDocument();
    // Plain "Called by ..." fallback should NOT render when preview is present.
    expect(document.body.textContent).not.toMatch(/called by/i);
  });

  it("provides a localized keyboard close button and keeps body-click dismissal", async () => {
    mockList.mockResolvedValue([recentRecall]);
    const first = render(<MilestoneToaster />, { wrapper });

    await screen.findByText(/wenlan just recalled for you/i);
    const close = screen.getByRole("button", { name: "Close" });
    expect(close).toHaveAttribute("aria-label", "Close");
    expect(close).toHaveAttribute("title", expect.stringContaining("Close"));
    expect(close.closest("button")?.parentElement?.closest("button")).toBeNull();

    fireEvent.click(close);
    await waitFor(() => expect(mockAcknowledge).toHaveBeenCalledWith("first-recall"));
    await waitFor(() =>
      expect(screen.queryByText(/wenlan just recalled for you/i)).not.toBeInTheDocument(),
    );
    first.unmount();

    mockAcknowledge.mockClear();
    mockList.mockResolvedValue([recentRecall]);
    render(<MilestoneToaster />, { wrapper });
    const body = await screen.findByText(/wenlan just recalled for you/i);
    fireEvent.click(body);
    await waitFor(() => expect(mockAcknowledge).toHaveBeenCalledWith("first-recall"));
  });

  it("raises the card in the app's one toast stack", async () => {
    mockList.mockResolvedValue([recentRecall]);
    render(<MilestoneToaster />, { wrapper });

    const card = await screen.findByTestId("milestone-toast");
    expect(card.closest("[data-sonner-toast]")?.className).toContain("mem-toast");
  });

  it("hides each toast after eight seconds without acknowledging it", async () => {
    useFakeTimers();
    renderWithSeededMilestones([recentRecall]);
    await settle();

    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();
    await advance(7_999);
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();
    await advance(1);
    await settleExit();
    expect(screen.queryByText(/wenlan just recalled for you/i)).not.toBeInTheDocument();
    expect(mockAcknowledge).not.toHaveBeenCalled();
  });

  it("times stacked toasts independently while hover pauses one toast", async () => {
    useFakeTimers();
    const firstMemory: MilestoneRecord = {
      id: "first-memory",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: null,
    };
    const secondAgent: MilestoneRecord = {
      id: "second-agent",
      first_triggered_at: Math.floor(Date.now() / 1000) - 5,
      acknowledged_at: null,
      payload: { agent: "cursor" },
    };
    renderWithSeededMilestones([firstMemory, secondAgent]);
    await settle();
    const memoryCard = screen.getByText(/first memory saved/i).closest("[data-testid='milestone-toast']")!;

    fireEvent.mouseEnter(memoryCard);
    await advance(8_000);
    await settleExit();

    expect(screen.getByText(/first memory saved/i)).toBeInTheDocument();
    expect(screen.queryByText(/A second AI/i)).not.toBeInTheDocument();
    fireEvent.mouseLeave(memoryCard);
    await advance(8_000);
    await settleExit();
    expect(screen.queryByText(/first memory saved/i)).not.toBeInTheDocument();
  });

  it("pauses while focused and resumes after focus leaves", async () => {
    useFakeTimers();
    renderWithSeededMilestones([recentRecall]);
    await settle();
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();

    const close = screen.getByRole("button", { name: "Close" });
    fireEvent.focus(close);
    await advance(8_000);
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();
    fireEvent.blur(close);
    await advance(7_999);
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();
    await advance(1);
    await settleExit();
    expect(screen.queryByText(/wenlan just recalled for you/i)).not.toBeInTheDocument();
  });

  it("pauses while the document is hidden and resumes remaining time", async () => {
    useFakeTimers();
    renderWithSeededMilestones([recentRecall]);
    await settle();
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();

    await advance(3_000);
    Object.defineProperty(document, "hidden", { configurable: true, value: true });
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    await advance(20_000);
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();

    Object.defineProperty(document, "hidden", { configurable: true, value: false });
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    await advance(4_999);
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();
    await advance(1);
    await settleExit();
    expect(screen.queryByText(/wenlan just recalled for you/i)).not.toBeInTheDocument();
  });

  it("does not reset a timer on rerender and treats a new trigger time as fresh", async () => {
    useFakeTimers();
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let currentRecords = [recentRecall];
    mockList.mockImplementation(() => Promise.resolve(currentRecords));
    queryClient.setQueryData(["onboarding-milestones"], [recentRecall]);
    const tree = () => (
      <QueryClientProvider client={queryClient}>
        <AppToaster />
        <MilestoneToaster />
      </QueryClientProvider>
    );
    const view = render(tree());
    await settle();
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();

    await advance(4_000);
    view.rerender(tree());
    await advance(4_000);
    await settleExit();
    expect(screen.queryByText(/wenlan just recalled for you/i)).not.toBeInTheDocument();

    const refired: MilestoneRecord = {
      ...recentRecall,
      first_triggered_at: recentRecall.first_triggered_at + 1,
    };
    currentRecords = [refired];
    await act(async () => {
      await queryClient.refetchQueries({ queryKey: ["onboarding-milestones"] });
    });
    view.rerender(tree());
    await settle();
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();
    await advance(7_999);
    expect(screen.getByText(/wenlan just recalled for you/i)).toBeInTheDocument();

    currentRecords = [recentRecall];
    await act(async () => {
      await queryClient.refetchQueries({ queryKey: ["onboarding-milestones"] });
    });
    view.rerender(tree());
    await settleExit();
    expect(screen.queryByText(/wenlan just recalled for you/i)).not.toBeInTheDocument();
    expect(mockAcknowledge).not.toHaveBeenCalled();
  });

  it("excludes acknowledged, old, and non-toast-channel milestones", async () => {
    const acknowledged: MilestoneRecord = {
      ...recentRecall,
      acknowledged_at: Math.floor(Date.now() / 1000),
    };
    const nonToast: MilestoneRecord = {
      ...recentRecall,
      id: "first-concept",
    };
    mockList.mockResolvedValue([recentRecall, acknowledged, oldRecall, nonToast]);
    render(<MilestoneToaster />, { wrapper });
    await screen.findByText(/wenlan just recalled for you/i);
    expect(screen.getAllByTestId("milestone-toast")).toHaveLength(1);
    expect(screen.queryByText(/first concept/i)).not.toBeInTheDocument();
  });
});
