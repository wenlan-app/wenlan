// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LookData, PageLook } from "../../../../lib/tauri";

const mocks = vi.hoisted(() => ({
  getPageLook: vi.fn(),
  getPageLookData: vi.fn(),
  reportPageLookError: vi.fn(),
}));

vi.mock("../../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../../lib/tauri")>()),
  getPageLook: mocks.getPageLook,
  getPageLookData: mocks.getPageLookData,
  reportPageLookError: mocks.reportPageLookError,
}));

import { LookFrame, LOOK_READY_TIMEOUT_MS } from "./LookFrame";
import { LookView } from "./LookView";
import { LookPencil } from "./LookPencil";
import { loadingKey, loadingValue, usePageLook } from "./usePageLook";

const DATA: LookData = {
  api: 1,
  page: { id: "pg_a", title: "A", space: "", created: "", modified: "", text: "", html: "", sections: [], links: [{ title: "B", id: "pg_b" }] },
  space: { name: "", pages: [], more: false },
  theme: { mode: "dark" },
};
const LOOK: PageLook = { id: 7, html: "<p>hi</p>", api: 1, kept_at: 1 };

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: React.ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

/** What the outer frame would post to the app. */
function fromFrame(frame: HTMLIFrameElement, body: unknown, source: unknown = frame.contentWindow) {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data: { wenlanLook: body }, source: source as MessageEventSource }));
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  mocks.getPageLookData.mockResolvedValue(DATA);
  mocks.reportPageLookError.mockResolvedValue({ recorded: true });
});
afterEach(() => { vi.useRealTimers(); });

describe("UX-LOOK-4 the frame", () => {
  it("mounts one sandboxed frame with scripts only and the look's document", () => {
    render(<LookFrame data={DATA} html="<p>hi</p>" title="A" />);
    const frame = screen.getByTestId("page-look-frame") as HTMLIFrameElement;
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("srcdoc")).toContain("Content-Security-Policy");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
  });

  it("hears only its own frame, and opens only pages it was given", () => {
    const onOpenPage = vi.fn();
    const onReady = vi.fn();
    render(<LookFrame data={DATA} html="" title="A" onOpenPage={onOpenPage} onReady={onReady} />);
    const frame = screen.getByTestId("page-look-frame") as HTMLIFrameElement;
    fromFrame(frame, { kind: "open", pageId: "pg_b" }, window);
    fromFrame(frame, { kind: "ready" }, window);
    expect(onOpenPage).not.toHaveBeenCalled();
    expect(onReady).not.toHaveBeenCalled();
    fromFrame(frame, { kind: "open", pageId: "pg_unknown" });
    expect(onOpenPage).not.toHaveBeenCalled();
    fromFrame(frame, { kind: "open", pageId: "pg_b" });
    expect(onOpenPage).toHaveBeenCalledWith("pg_b");
    fromFrame(frame, { kind: "ready" });
    expect(onReady).toHaveBeenCalledTimes(1);
  });

  it("sizes the frame from the height it reports", () => {
    render(<LookFrame data={DATA} html="" title="A" />);
    const frame = screen.getByTestId("page-look-frame") as HTMLIFrameElement;
    fromFrame(frame, { kind: "height", value: 777 });
    expect(frame.style.height).toBe("777px");
  });

  it("calls an error before ready a failure, and one after ready not", () => {
    const onError = vi.fn();
    render(<LookFrame data={DATA} html="" title="A" onError={onError} />);
    const frame = screen.getByTestId("page-look-frame") as HTMLIFrameElement;
    fromFrame(frame, { kind: "error", message: "boom" });
    expect(onError).toHaveBeenLastCalledWith("boom", true);
    fromFrame(frame, { kind: "ready" });
    fromFrame(frame, { kind: "error", message: "later" });
    expect(onError).toHaveBeenLastCalledWith("later", false);
  });

  it("gives up when no ready comes in three seconds", () => {
    vi.useFakeTimers();
    const onError = vi.fn();
    render(<LookFrame data={DATA} html="" title="A" onError={onError} />);
    act(() => { vi.advanceTimersByTime(LOOK_READY_TIMEOUT_MS - 1); });
    expect(onError).not.toHaveBeenCalled();
    act(() => { vi.advanceTimersByTime(2); });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][1]).toBe(true);
  });

  it("does not blame the look while the app is hidden", () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    const onError = vi.fn();
    render(<LookFrame data={DATA} html="" title="A" onError={onError} />);
    act(() => { vi.advanceTimersByTime(LOOK_READY_TIMEOUT_MS * 3); });
    expect(onError).not.toHaveBeenCalled();
    visibility.mockReturnValue("visible");
    fromFrame(screen.getByTestId("page-look-frame") as HTMLIFrameElement, { kind: "ready" });
    act(() => { vi.advanceTimersByTime(LOOK_READY_TIMEOUT_MS * 2); });
    expect(onError).not.toHaveBeenCalled();
    visibility.mockRestore();
  });

  it("does not time out once ready", () => {
    vi.useFakeTimers();
    const onError = vi.fn();
    render(<LookFrame data={DATA} html="" title="A" onError={onError} />);
    fromFrame(screen.getByTestId("page-look-frame") as HTMLIFrameElement, { kind: "ready" });
    act(() => { vi.advanceTimersByTime(10_000); });
    expect(onError).not.toHaveBeenCalled();
  });
});

describe("UX-LOOK-3 a look that cannot be shown falls back to the text", () => {
  it("reports an error before ready to the daemon and asks for the text", async () => {
    const onFailed = vi.fn();
    render(<LookView pageId="pg_a" title="A" look={LOOK} version="1" onFailed={onFailed} />, { wrapper: wrapper() });
    const frame = (await screen.findByTestId("page-look-frame")) as HTMLIFrameElement;
    expect(window.localStorage.getItem(loadingKey("pg_a"))).toBe(loadingValue(7));
    fromFrame(frame, { kind: "error", message: "boom" });
    expect(onFailed).toHaveBeenCalledTimes(1);
    expect(mocks.reportPageLookError).toHaveBeenCalledWith("pg_a", "boom");
    expect(window.localStorage.getItem(loadingKey("pg_a"))).toBeNull();
  });

  it("keeps showing a look that errors after it is ready, and tells the daemon", async () => {
    const onFailed = vi.fn();
    render(<LookView pageId="pg_a" title="A" look={LOOK} version="1" onFailed={onFailed} />, { wrapper: wrapper() });
    const frame = (await screen.findByTestId("page-look-frame")) as HTMLIFrameElement;
    fromFrame(frame, { kind: "ready" });
    fromFrame(frame, { kind: "error", message: "late" });
    expect(onFailed).not.toHaveBeenCalled();
    expect(mocks.reportPageLookError).toHaveBeenCalledWith("pg_a", "late");
    expect(window.localStorage.getItem(loadingKey("pg_a"))).toBeNull();
  });

  it("asks for the text when the page's data cannot be read", async () => {
    mocks.getPageLookData.mockRejectedValue(new Error("daemon down"));
    const onFailed = vi.fn();
    render(<LookView pageId="pg_a" title="A" look={LOOK} version="1" onFailed={onFailed} />, { wrapper: wrapper() });
    await waitFor(() => expect(onFailed).toHaveBeenCalled());
    expect(screen.queryByTestId("page-look-frame")).toBeNull();
  });

  it("clears its loading mark when it goes away", async () => {
    const view = render(<LookView pageId="pg_a" title="A" look={LOOK} version="1" onFailed={vi.fn()} />, { wrapper: wrapper() });
    expect(window.localStorage.getItem(loadingKey("pg_a"))).toBe(loadingValue(7));
    view.unmount();
    expect(window.localStorage.getItem(loadingKey("pg_a"))).toBeNull();
  });
});

function Probe({ version = "1", supported = true }: { version?: string; supported?: boolean }) {
  const look = usePageLook({ pageId: "pg_a", supported, version });
  return (
    <div>
      <span data-testid="state">{JSON.stringify({ has: look.hasLook, mode: look.mode, shown: look.shown, failed: look.failed })}</span>
      <button onClick={() => look.choose("text")}>text</button>
      <button onClick={() => look.choose("look")}>look</button>
      <button onClick={() => look.markFailed()}>fail</button>
    </div>
  );
}
const state = () => JSON.parse(screen.getByTestId("state").textContent ?? "{}");

describe("UX-LOOK-2 one switch, remembered per page", () => {
  it("shows no look, and no switch state, for a page without one", async () => {
    mocks.getPageLook.mockResolvedValue({ look: null, versions: [] });
    render(<Probe />, { wrapper: wrapper() });
    await waitFor(() => expect(mocks.getPageLook).toHaveBeenCalled());
    expect(state()).toMatchObject({ has: false, shown: false });
  });

  it("does not even ask for a page that takes no look", () => {
    render(<Probe supported={false} />, { wrapper: wrapper() });
    expect(mocks.getPageLook).not.toHaveBeenCalled();
    expect(state().has).toBe(false);
  });

  it("opens on the look every time, even after the text was shown", async () => {
    mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    const view = render(<Probe />, { wrapper: wrapper() });
    await waitFor(() => expect(state().has).toBe(true));
    expect(state()).toMatchObject({ mode: "look", shown: true });
    fireEvent.click(screen.getByText("text"));
    expect(state()).toMatchObject({ mode: "text", shown: false });
    view.unmount();
    render(<Probe />, { wrapper: wrapper() });
    await waitFor(() => expect(state().has).toBe(true));
    expect(state()).toMatchObject({ mode: "look", shown: true });
  });

  it("falls back to Text on a failure, and choosing Look tries again", async () => {
    mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    render(<Probe />, { wrapper: wrapper() });
    await waitFor(() => expect(state().has).toBe(true));
    fireEvent.click(screen.getByText("fail"));
    expect(state()).toMatchObject({ failed: true, shown: false });
    fireEvent.click(screen.getByText("look"));
    expect(state()).toMatchObject({ failed: false, shown: true });
  });

  it("forgets a failure when the page's text changes", async () => {
    mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    const view = render(<Probe version="1" />, { wrapper: wrapper() });
    await waitFor(() => expect(state().has).toBe(true));
    fireEvent.click(screen.getByText("fail"));
    expect(state().failed).toBe(true);
    view.rerender(<Probe version="2" />);
    expect(state()).toMatchObject({ failed: false, shown: true });
  });

  it("does not open a look by itself that was loading when the app last stopped", async () => {
    window.localStorage.setItem(loadingKey("pg_a"), "7:an-earlier-run");
    mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    render(<Probe />, { wrapper: wrapper() });
    await waitFor(() => expect(state().failed).toBe(true));
    expect(state().shown).toBe(false);
    expect(window.localStorage.getItem(loadingKey("pg_a"))).toBeNull();
    fireEvent.click(screen.getByText("look"));
    expect(state().shown).toBe(true);
  });

  it("ignores a stale mark left by an earlier look", async () => {
    window.localStorage.setItem(loadingKey("pg_a"), "3:an-earlier-run");
    mocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    render(<Probe />, { wrapper: wrapper() });
    await waitFor(() => expect(state().has).toBe(true));
    expect(state()).toMatchObject({ failed: false, shown: true });
  });
});

describe("the pencil", () => {
  it("UX-LOOK-2 is one pencil: pressed shows the text, pressed again brings the design back", () => {
    const onChange = vi.fn();
    const view = render(<LookPencil editing={false} onChange={onChange} />);
    const pencil = screen.getByRole("button", { name: "Edit page" });
    expect(pencil.getAttribute("aria-pressed")).toBe("false");
    expect(pencil.textContent).toBe("");
    fireEvent.click(pencil);
    expect(onChange).toHaveBeenLastCalledWith("text");
    view.rerender(<LookPencil editing onChange={onChange} />);
    expect(screen.getByRole("button", { name: "Edit page" }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Edit page" }));
    expect(onChange).toHaveBeenLastCalledWith("look");
  });
});

describe("UX-LOOK-9 a look that names sites", () => {
  const SITES_LOOK: PageLook = {
    ...LOOK,
    html: '<p>weather</p><script type="application/json" id="wenlan-sites">["api.open-meteo.com"]</script>',
  };

  it("runs offline and asks once; Allow lets only that site in, and is remembered for the page", async () => {
    const view = render(<LookView pageId="pg_a" title="A" look={SITES_LOOK} version={1} onFailed={vi.fn()} />, { wrapper: wrapper() });
    const frame = await screen.findByTestId("page-look-frame");
    expect(frame.getAttribute("srcdoc")).toContain("connect-src 'none'");
    expect(frame.getAttribute("srcdoc")).not.toContain("https://api.open-meteo.com");
    expect(screen.getByRole("status").textContent).toContain("api.open-meteo.com");
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(screen.getByTestId("page-look-frame").getAttribute("srcdoc")).toContain("connect-src https://api.open-meteo.com"));
    expect(screen.queryByRole("status")).toBeNull();
    view.unmount();

    render(<LookView pageId="pg_a" title="A" look={SITES_LOOK} version={1} onFailed={vi.fn()} />, { wrapper: wrapper() });
    expect((await screen.findByTestId("page-look-frame")).getAttribute("srcdoc")).toContain("connect-src https://api.open-meteo.com");
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("Not now keeps it offline and stops asking for now", async () => {
    render(<LookView pageId="pg_c" title="C" look={SITES_LOOK} version={1} onFailed={vi.fn()} />, { wrapper: wrapper() });
    await screen.findByTestId("page-look-frame");
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByTestId("page-look-frame").getAttribute("srcdoc")).not.toContain("https://api.open-meteo.com");
    expect(window.localStorage.getItem("wenlan-look-sites:pg_c")).toBeNull();
  });
});
