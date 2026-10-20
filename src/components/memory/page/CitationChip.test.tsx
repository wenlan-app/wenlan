// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, fireEvent, act, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { open as shellOpen } from "@tauri-apps/plugin-shell";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { MemoryItem, PageCitation } from "../../../lib/tauri";
import CitationChip from "./CitationChip";
import { citationFilePath } from "./CitationPopover";

const cite = (over: Partial<PageCitation> = {}): PageCitation => ({
  occurrence: 1,
  marker: 1,
  source_kind: "memory",
  locator: "mem-1",
  score: 0.9,
  status: "verified",
  scope: "sentence",
  ...over,
});

const memory = (over: Partial<MemoryItem> = {}): MemoryItem => ({
  source_id: "mem-1",
  title: "Design decision",
  content: "We decided to keep the daemon local-first because it is simpler.",
  summary: null,
  memory_type: "memory",
  domain: null,
  source_agent: "claude-code",
  confidence: null,
  confirmed: true,
  pinned: false,
  supersedes: null,
  last_modified: Math.floor(Date.now() / 1000),
  chunk_count: 1,
  ...over,
});

function renderChip(over: Partial<React.ComponentProps<typeof CitationChip>> = {}) {
  const onOpenMemory = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={client}>
      <CitationChip
        occurrence={1}
        citation={cite()}
        sourceMemory={memory()}
        sourcesLoading={false}
        onOpenMemory={onOpenMemory}
        {...over}
      />
    </QueryClientProvider>,
  );
  return { onOpenMemory, ...utils };
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(shellOpen).mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("CitationChip", () => {
  it("keeps touch in the preview even when focus has already opened it", () => {
    const { onOpenMemory } = renderChip();
    const chip = screen.getByRole("button", { name: /Memory/ });
    const down = new MouseEvent("pointerdown", { bubbles: true, button: 0 });
    Object.defineProperty(down, "pointerType", { value: "touch" });
    fireEvent(chip, down);
    fireEvent.focus(chip);
    fireEvent.click(chip);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Design decision");
    expect(onOpenMemory).not.toHaveBeenCalled();
  });

  it("renders a focusable button with localized source kind and occurrence superscript", () => {
    renderChip();
    const chip = screen.getByRole("button", { name: /Memory/ });
    expect(chip).toBeInTheDocument();
    expect(chip.querySelector("sup")?.textContent).toBe("1");
    expect(chip).toHaveAttribute("data-status", "verified");
  });

  it("does not expose an imported memory locator in the chip", () => {
    const locator = "import_20260908T150227Z_0_0";
    renderChip({
      citation: cite({ locator }),
      sourceMemory: memory({
        source_id: locator,
        title: "Imported project context",
      }),
    });
    const chip = screen.getByRole("button", { name: /Memory/ });
    expect(chip).not.toHaveTextContent(locator);
    fireEvent.focus(chip);
    const popover = screen.getByRole("tooltip");
    expect(popover).toHaveTextContent("Imported project context");
    expect(popover).toHaveTextContent(locator);
    expect(popover).toHaveTextContent("Technical details");
  });

  it("marks unverified citations", () => {
    renderChip({ citation: cite({ status: "unverified" }) });
    expect(screen.getByRole("button", { name: /Memory/ })).toHaveAttribute(
      "data-status",
      "unverified",
    );
  });

  it("opens the popover on focus and links it via aria-describedby", () => {
    renderChip();
    const chip = screen.getByRole("button", { name: /Memory/ });
    fireEvent.focus(chip);
    const tip = screen.getByRole("tooltip");
    expect(tip).toBeInTheDocument();
    expect(chip.getAttribute("aria-describedby")).toBe(tip.getAttribute("id"));
  });

  it("portals the popover out of article prose and keeps keyboard focus open", async () => {
    const user = userEvent.setup();
    const onOpenMemory = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <p data-testid="article-prose">
          A cited sentence
          <CitationChip
            occurrence={1}
            citation={cite()}
            sourceMemory={memory()}
            sourcesLoading={false}
            onOpenMemory={onOpenMemory}
          />
          .
          <button>Next article action</button>
        </p>
      </QueryClientProvider>,
    );

    const article = screen.getByTestId("article-prose");
    const chip = screen.getByRole("button", { name: /Memory/ });
    await act(async () => chip.focus());
    expect(document.activeElement).toBe(chip);
    const popover = await screen.findByRole("tooltip");
    expect(popover.parentElement).toBe(document.body);
    expect(article).not.toContainElement(popover);

    const openMemory = within(popover).getByRole("button", { name: /Open memory/ });
    await user.tab();
    expect(document.activeElement).toBe(within(popover).getByText("Technical details"));
    await user.tab();
    expect(document.activeElement).toBe(openMemory);
    expect(screen.getByRole("tooltip")).toBeInTheDocument();
    await user.tab({ shift: true });
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(chip);
    await user.tab();
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(chip);
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Next article action" }));
    await user.tab({ shift: true });
    await user.tab();
    await user.tab();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Next article action" }));
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("closes the popover on Escape", () => {
    renderChip();
    const chip = screen.getByRole("button", { name: /Memory/ });
    fireEvent.focus(chip);
    expect(screen.getByRole("tooltip")).toBeInTheDocument();
    fireEvent.keyDown(chip, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("opens on hover after a delay and closes on mouse-out", () => {
    vi.useFakeTimers();
    renderChip();
    const wrapper = screen.getByRole("button", { name: /Memory/ }).parentElement!;
    fireEvent.mouseEnter(wrapper);
    expect(screen.queryByRole("tooltip")).toBeNull();
    act(() => vi.advanceTimersByTime(200));
    expect(screen.getByRole("tooltip")).toBeInTheDocument();
    fireEvent.mouseLeave(wrapper);
    act(() => vi.advanceTimersByTime(200));
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("shows memory details and opens the memory from the popover action", async () => {
    const user = userEvent.setup();
    const { onOpenMemory } = renderChip();
    fireEvent.focus(screen.getByRole("button", { name: /Memory/ }));
    const popover = screen.getByRole("tooltip");
    expect(within(popover).getByText("Design decision")).toBeInTheDocument();
    expect(within(popover).getByText("Memory")).toBeInTheDocument();
    expect(
      within(popover).getByText(/We decided to keep the daemon/),
    ).toBeInTheDocument();
    await user.click(within(popover).getByRole("button", { name: /Open memory/ }));
    expect(onOpenMemory).toHaveBeenCalledWith("mem-1");
  });

  it("clicking a memory chip opens the memory directly", async () => {
    const user = userEvent.setup();
    const { onOpenMemory } = renderChip();
    await user.click(screen.getByRole("button", { name: /Memory/ }));
    expect(onOpenMemory).toHaveBeenCalledWith("mem-1");
  });

  it("explains a missing source and does not navigate on click", async () => {
    const user = userEvent.setup();
    const { onOpenMemory } = renderChip({
      sourceMemory: null,
      sourcesLoading: false,
    });
    const chip = screen.getByRole("button", { name: /Memory/ });
    fireEvent.focus(chip);
    expect(chip).not.toHaveTextContent("mem-1");
    expect(screen.getByText(/no longer exists/i)).toBeInTheDocument();
    expect(screen.getByText(/update the page from its sources/i)).toBeInTheDocument();
    expect(screen.getByText("mem-1")).toBeInTheDocument();
    await user.click(chip);
    expect(onOpenMemory).not.toHaveBeenCalled();
  });

  it("shows a skeleton while sources are loading", () => {
    renderChip({ sourceMemory: null, sourcesLoading: true });
    fireEvent.focus(screen.getByRole("button", { name: /Memory/ }));
    expect(screen.getByTestId("citation-popover-skeleton")).toBeInTheDocument();
  });

  it("pages unverified status in the popover", () => {
    renderChip({ citation: cite({ status: "unverified" }) });
    fireEvent.focus(screen.getByRole("button", { name: /Memory/ }));
    expect(screen.getByText(/unverified/i)).toBeInTheDocument();
  });

  it("opens external urls in the browser", async () => {
    const user = userEvent.setup();
    renderChip({
      citation: cite({ source_kind: "external_url", locator: "https://docs.rs/serde" }),
      sourceMemory: null,
    });
    await user.click(screen.getByRole("button", { name: /docs\.rs/ }));
    expect(vi.mocked(shellOpen)).toHaveBeenCalledWith("https://docs.rs/serde");
  });

  it("shows the file path and opens it from the popover action", async () => {
    const user = userEvent.setup();
    renderChip({
      citation: cite({ source_kind: "external_file", locator: "/notes/design.md" }),
      sourceMemory: null,
    });
    fireEvent.focus(screen.getByRole("button", { name: /File.*design\.md/ }));
    expect(screen.getByText("/notes/design.md")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Open file/ }));
    // A bare path must not go to the shell plugin: its `open` validator only
    // admits URL schemes, so the app's own `open_file` command opens files.
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("open_file", { path: "/notes/design.md" });
    expect(vi.mocked(shellOpen)).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows why a file could not be opened instead of failing silently", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockRejectedValueOnce(
      'Refusing to run "/notes/run.command". Wenlan opens documents and folders, not programs.',
    );
    renderChip({
      citation: cite({ source_kind: "external_file", locator: "/notes/run.command" }),
      sourceMemory: null,
    });
    fireEvent.focus(screen.getByRole("button", { name: /File.*run\.command/ }));
    await user.click(screen.getByRole("button", { name: /Open file/ }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not open this file.");
    expect(alert).toHaveTextContent(/Refusing to run/);
  });

  it("shows why a link could not be opened from the popover action", async () => {
    const user = userEvent.setup();
    vi.mocked(shellOpen).mockRejectedValueOnce(new Error("scope validation failed"));
    renderChip({
      citation: cite({ source_kind: "external_url", locator: "https://docs.rs/serde" }),
      sourceMemory: null,
    });
    fireEvent.focus(screen.getByRole("button", { name: /Link.*docs\.rs/ }));
    await user.click(screen.getByRole("button", { name: /Open in browser/ }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not open this link.");
    expect(alert).toHaveTextContent("scope validation failed");
  });

  it("opens the popover with the reason when a chip click on a link is refused", async () => {
    const user = userEvent.setup();
    vi.mocked(shellOpen).mockRejectedValueOnce(new Error("scope validation failed"));
    renderChip({
      citation: cite({ source_kind: "external_url", locator: "https://docs.rs/serde" }),
      sourceMemory: null,
    });
    await user.click(screen.getByRole("button", { name: /Link.*docs\.rs/ }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not open this link.");
    expect(screen.getByRole("tooltip")).toContainElement(alert);
  });

  it("shows only the path of a document citation whose locator carries the source id", async () => {
    const user = userEvent.setup();
    renderChip({
      citation: cite({
        source_kind: "external_file",
        locator: "directory-notes::/Users/me/Tally/notes/architecture-notes.md",
      }),
      sourceMemory: null,
    });
    fireEvent.focus(screen.getByRole("button", { name: /File.*architecture-notes\.md/ }));
    expect(screen.getByText("/Users/me/Tally/notes/architecture-notes.md")).toBeInTheDocument();
    expect(screen.queryByText(/directory-notes::/)).toBeNull();
    await user.click(screen.getByRole("button", { name: /Open file/ }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("open_file", {
      path: "/Users/me/Tally/notes/architecture-notes.md",
    });
  });

  it("explains that authored content survives a rebuild", () => {
    renderChip({ citation: cite({ source_kind: "authored" }), sourceMemory: null });
    fireEvent.focus(screen.getByRole("button", { name: /Written here/ }));
    expect(screen.getByText(/kept unchanged when the page is rebuilt/i)).toBeInTheDocument();
  });
});


describe("citationFilePath", () => {
  it("strips a document source id prefix and leaves plain locators alone", () => {
    expect(citationFilePath("directory-notes::/Users/me/notes/a.md")).toBe("/Users/me/notes/a.md");
    expect(citationFilePath("obsidian-vault::C:\\Users\\me\\vault\\a.md")).toBe("C:\\Users\\me\\vault\\a.md");
    expect(citationFilePath("/notes/design.md")).toBe("/notes/design.md");
    expect(citationFilePath("https://docs.rs/serde")).toBe("https://docs.rs/serde");
  });
});
