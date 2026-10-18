// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import * as api from "../../lib/tauri";
import ReviewDialog, { reviewApproveBlocked, reviewKindLabel } from "./ReviewDialog";
import { reviewItemId, type ReviewItem } from "./useReviewQueue";
import { createSpacesNavigationFixture } from "../../../e2e/fixtures/spacesNavigation";

vi.mock("../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../lib/tauri")>()),
  getPage: vi.fn(), getPageSources: vi.fn(), getMemoryDetail: vi.fn(), getEntityDetail: vi.fn(),
  getPageRevisions: vi.fn(), search: vi.fn(),
  clipboardWrite: vi.fn(),
  repairRecovery: vi.fn(),
}));
const fixture = createSpacesNavigationFixture();
const archive: ReviewItem = {
  kind: "refinement", id: "archive", action: "page_keep_or_archive",
  sourceIds: [fixture.pages[0].id], payload: { action: "page_keep_or_archive", page_id: fixture.pages[0].id, source_count: 1 },
  confidence: 0.8, timestampMs: 0,
};
const entity: ReviewItem = {
  ...archive, id: "entity", action: "entity_merge",
  sourceIds: [fixture.entities[1].id, fixture.entities[0].id],
  payload: { action: "entity_merge", new_id: fixture.entities[1].id, existing_id: fixture.entities[0].id, similarity: 0.9 },
};
const merge: ReviewItem = { ...archive, id: "merge", action: "page_merge", sourceIds: [fixture.pages[0].id, fixture.pages[1].id], payload: null };
function mount(item: ReviewItem) {
  const resolve = vi.fn().mockResolvedValue(undefined);
  const onOpenPage = vi.fn(); const onOpenMemory = vi.fn();
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <ReviewDialog items={[item]} openId={reviewItemId(item)} onOpenChange={vi.fn()} onResolve={resolve}
      isResolving={false} onOpenPage={onOpenPage} onOpenMemory={onOpenMemory} />
  </QueryClientProvider>);
  return { resolve, onOpenPage, onOpenMemory, user: userEvent.setup() };
}
beforeEach(async () => {
  vi.clearAllMocks();
  await i18n.changeLanguage("en");
  vi.mocked(api.getPage).mockImplementation(async (id) => fixture.pages.find((page) => page.id === id) ?? null);
  vi.mocked(api.getPageSources).mockResolvedValue([]);
  vi.mocked(api.getMemoryDetail).mockImplementation(async (id) => fixture.memories.find((memory) => memory.source_id === id) ?? null);
  vi.mocked(api.getEntityDetail).mockImplementation(async (id) => fixture.entityDetails.find((detail) => detail.entity.id === id)!);
  vi.mocked(api.getPageRevisions).mockResolvedValue({ page_id: fixture.pages[0].id, current_version: 1, user_edited: false, entries: [] });
  vi.mocked(api.search).mockResolvedValue([]);
  vi.mocked(api.clipboardWrite).mockResolvedValue(undefined);
  vi.mocked(api.repairRecovery).mockResolvedValue(null);
});

describe("Review decision contracts", () => {
  it("labels declining a source repair as keeping the source and never approves it", async () => {
    const item: ReviewItem = {
      ...archive, id: "repair-keep", action: "lint_repair_review",
      payload: { action: "lint_repair_review", check_id: "unsupported.check",
        occurrence_digest: "a".repeat(64), owner_binding_digest: "b".repeat(64),
        issue: "Inspect the source", choices: ["Keep current source"], suggested_research_queries: [] },
    };
    const { resolve, user } = mount(item);
    resolve.mockRejectedValueOnce(new Error('HTTP POST /api/refinery/queue/repair-keep/reject returned 409: {"error":"repair_write_fence_conflict"}'));
    await user.click(screen.getByRole("button", { name: "Keep current source" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.sourceBusy"));
    expect(screen.queryByText("repair_write_fence_conflict")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Keep current source" }));
    expect(resolve).toHaveBeenCalledTimes(2);
    expect(resolve).toHaveBeenCalledWith({ item, approve: false });
  });

  it("retains the active repair when a queue refresh removes its row", async () => {
    vi.mocked(api.getPage).mockImplementation(() => new Promise(() => {}));
    const item: ReviewItem = {
      ...archive, id: "repair-pending", action: "lint_repair_review",
      payload: { action: "lint_repair_review", check_id: "pages.duplicate_active_titles",
        occurrence_digest: "a".repeat(64), owner_binding_digest: "b".repeat(64),
        issue: "Keep this repair available during recovery", choices: ["Rename"], suggested_research_queries: [] },
    };
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onOpenChange = vi.fn();
    const content = (items: ReviewItem[]) => <QueryClientProvider client={client}>
      <ReviewDialog items={items} openId={reviewItemId(item)} onOpenChange={onOpenChange}
        onResolve={vi.fn()} isResolving={false} />
    </QueryClientProvider>;
    const view = render(content([item, archive]));
    await waitFor(() => expect(screen.getByRole("button", { name: "Close" })).toBeDisabled());
    expect(screen.getByRole("button", { name: "Skip" })).toBeDisabled();
    await userEvent.setup().click(screen.getByRole("button", { name: "Skip" }));
    screen.getByRole("dialog").querySelector<HTMLElement>('[tabindex="-1"]')?.focus();
    await userEvent.setup().keyboard("{ArrowRight}");
    expect(onOpenChange).not.toHaveBeenCalled();
    view.rerender(content([]));
    expect(screen.queryByRole("button", { name: "Skip" })).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t("sourceRepair.duplicateHint"))).toBeVisible();
    expect(screen.getByText(i18n.t("sourceRepair.inspecting"))).toBeVisible();
    expect(screen.getByRole("button", { name: "Close" })).toBeDisabled();
    await userEvent.setup().keyboard("{Escape}");
    expect(onOpenChange).not.toHaveBeenCalled();
  });

  it.each(["lint_repair_review", "unknown", "future_action", "suggest_entity", "dedup_merge", "cross_space_discovery"])("labels and blocks unsupported %s, including keyboard activation", async (action) => {
    const item = { ...archive, action, payload: action === "vocab_promote" ? { action, kind: "entity", old_value: "research-method" } : null } as ReviewItem;
    const { resolve, user } = mount(item);
    expect(reviewKindLabel(i18n.t, item)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("cannot be approved here");
    if (action === "vocab_promote") expect(screen.getByText(/research-method/)).toBeVisible();
    await user.keyboard("{Enter}");
    expect(resolve).not.toHaveBeenCalled();
  });

  const vocabulary: ReviewItem = { ...archive, id: "vocab", action: "vocab_promote", sourceIds: [fixture.entities[0].id], payload: { action: "vocab_promote", kind: "entity", old_value: "Research-Method", category: "concept" } };

  it("shows vocabulary impact and current entity types before accepting", async () => {
    const { resolve, user } = mount(vocabulary);
    const button = screen.getByRole("button", { name: "Add to vocabulary" });
    await waitFor(() => expect(button).toBeEnabled());
    expect(screen.getByText(/Proposed vocabulary: research-method/)).toBeVisible();
    expect(screen.getByText(/Ada Lovelace · Current type: person/)).toBeVisible();
    expect(screen.getByText(/associated active entities still typed concept/)).toBeVisible();
    screen.getByRole("dialog").focus();
    await user.keyboard("{Enter}");
    expect(resolve).not.toHaveBeenCalled();
    await user.click(button);
    expect(resolve).toHaveBeenCalledWith({ item: vocabulary, approve: true });
  });

  it("blocks vocabulary approval on a failed entity lookup until retry succeeds", async () => {
    vi.mocked(api.getEntityDetail).mockRejectedValue(new Error("offline"));
    const { user, resolve } = mount(vocabulary);
    expect(await screen.findByRole("alert")).toBeVisible();
    const button = screen.getByRole("button", { name: "Add to vocabulary" });
    expect(button).toBeDisabled();
    await user.click(button); expect(resolve).not.toHaveBeenCalled();
    vi.mocked(api.getEntityDetail).mockResolvedValue(fixture.entityDetails[0]);
    await user.click(screen.getByRole("button", { name: "Retry loading" }));
    await waitFor(() => expect(button).toBeEnabled());
  });

  it("permits relation vocabulary without pretending to rewrite existing relations", async () => {
    const item = { ...vocabulary, sourceIds: [], payload: { action: "vocab_promote", kind: "relation", old_value: "informed_by" } } as ReviewItem;
    const { user, resolve } = mount(item);
    expect(screen.getByText(/Existing relations will not be rewritten/)).toBeVisible();
    expect(api.getEntityDetail).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Add to vocabulary" }));
    expect(resolve).toHaveBeenCalledWith({ item, approve: true });
  });

  it("does not accept vocabulary when an entity response belongs to another id", async () => {
    vi.mocked(api.getEntityDetail).mockResolvedValue(fixture.entityDetails[1]);
    mount(vocabulary);
    expect(await screen.findByRole("alert")).toHaveTextContent("no longer available");
    expect(screen.getByRole("button", { name: "Add to vocabulary" })).toBeDisabled();
  });

  it("copies the complete repair binding without resolving the proposal and reports copy failures", async () => {
    const payload: api.RefinementPayload = { action: "lint_repair_review", check_id: "check", occurrence_digest: "a".repeat(64), owner_binding_digest: "b".repeat(64), issue: "Source changed", choices: ["Research source"], suggested_research_queries: ["Find the original wording"] };
    const item: ReviewItem = { ...archive, id: "repair", action: "lint_repair_review", payload };
    vi.mocked(api.clipboardWrite).mockRejectedValueOnce(new Error("denied"));
    const { resolve, user } = mount(item);
    await user.click(screen.getByText(i18n.t("sourceRepair.showDetails"), { selector: "summary" }));
    const button = screen.getByRole("button", { name: "Copy repair details" });
    expect(screen.getByText("Find the original wording")).toBeVisible();
    await user.click(button);
    expect(await screen.findByText(/Could not copy/)).toBeVisible();
    await user.click(button);
    expect(await screen.findByText("Repair details copied. The proposal remains pending.")).toBeVisible();
    expect(JSON.parse(vi.mocked(api.clipboardWrite).mock.calls[1][0])).toEqual({ review_id: "repair", source_ids: item.sourceIds, ...payload });
    expect(resolve).not.toHaveBeenCalled();
  });

  it.each([
    { action: "vocab_promote", kind: "future-kind", old_value: "example" },
    { action: "vocab_promote", kind: "entity", old_value: " " },
    { action: "vocab_promote", kind: "entity", old_value: " example " },
    null,
  ])("blocks malformed vocabulary payloads", (payload) => {
    expect(reviewApproveBlocked({ ...vocabulary, payload } as ReviewItem)).toBe(true);
  });

  it.each([
    { ...archive, sourceIds: ["a-memory-id"] },
    { ...archive, payload: null },
    { ...entity, sourceIds: [...entity.sourceIds].reverse() },
    { ...entity, sourceIds: ["same", "same"] },
    { ...entity, payload: null },
    { ...merge, sourceIds: [fixture.pages[0].id] },
    { ...merge, payload: { action: "page_merge", left_page_id: fixture.pages[1].id, right_page_id: fixture.pages[0].id, source_overlap: 0, source_overlap_ratio: 0 } },
  ] as ReviewItem[])("refuses incomplete or contradictory target information: $id $sourceIds", (item) => {
    expect(reviewApproveBlocked(item)).toBe(true);
  });

  it("shows asymmetric entity names in the daemon's survivor/incoming order", async () => {
    const { resolve, user } = mount(entity);
    const keep = await screen.findByText("Keeps");
    await waitFor(() => expect(keep.parentElement).toHaveTextContent("Ada Lovelace"));
    expect(screen.getByText("Folds in").parentElement).toHaveTextContent("Charles Babbage");
    const approve = screen.getByRole("button", { name: "Approve" });
    await waitFor(() => expect(approve).toBeEnabled());
    await user.click(approve);
    expect(resolve).toHaveBeenCalledWith({ item: entity, approve: true });
  });

  it("requires focus on the actual action button before Enter can archive", async () => {
    const { resolve, user } = mount(archive);
    const button = screen.getByRole("button", { name: "Archive" });
    await waitFor(() => expect(button).toBeEnabled());
    screen.getByRole("dialog").focus();
    await user.keyboard("{Enter}");
    expect(resolve).not.toHaveBeenCalled();
    button.focus(); await user.keyboard("{Enter}");
    expect(resolve).toHaveBeenCalledWith({ item: archive, approve: true });
  });

  it("blocks archive during loading, then permits it after the required evidence arrives", async () => {
    let finish!: (value: api.Page | null) => void;
    vi.mocked(api.getPage).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const { resolve, user } = mount(archive);
    const button = screen.getByRole("button", { name: "Archive" });
    expect(button).toBeDisabled();
    await user.click(button); expect(resolve).not.toHaveBeenCalled();
    finish(fixture.pages[0]);
    await waitFor(() => expect(button).toBeEnabled());
  });

  it("keeps archive disabled for a missing page", async () => {
    vi.mocked(api.getPage).mockResolvedValue(null);
    vi.mocked(api.getPageSources).mockRejectedValue(new Error("page not found"));
    const { resolve, user } = mount(archive);
    expect(await screen.findByRole("alert")).toHaveTextContent("required information");
    const button = screen.getByRole("button", { name: "Archive" });
    expect(button).toBeDisabled(); await user.click(button);
    expect(screen.queryByRole("button", { name: "Retry loading" })).toBeNull();
    expect(resolve).not.toHaveBeenCalled();
  });

  it.each([archive, merge])("blocks $action when sources fail, and supports a successful retry", async (item) => {
    vi.mocked(api.getPageSources).mockRejectedValue(new Error("offline"));
    const { resolve, user } = mount(item);
    const button = screen.getByRole("button", { name: item.action === "page_merge" ? "Merge notes" : "Archive" });
    expect(await screen.findByRole("alert")).toHaveTextContent("required information");
    expect(button).toBeDisabled(); await user.click(button); expect(resolve).not.toHaveBeenCalled();
    vi.mocked(api.getPageSources).mockResolvedValue([]);
    await user.click(screen.getByRole("button", { name: "Retry loading" }));
    await waitFor(() => expect(button).toBeEnabled());
  });

  it.each(["memory", "page"] as const)("blocks a %s revision when its before-target is missing", async (targetKind) => {
    const item: ReviewItem = { kind: "revision", id: "rev", targetKind, targetSourceId: "missing", revisionSourceId: "new", content: "New wording", agent: null, timestampMs: 0 };
    mount(item);
    expect(await screen.findByRole("alert")).toHaveTextContent("required information");
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  });

  it("distinguishes a failed evidence search from an empty result", async () => {
    vi.mocked(api.search).mockRejectedValue(new Error("offline"));
    const { user } = mount({ kind: "topic", id: "topic", label: "Evidence topic", count: 2, timestampMs: null });
    expect(await screen.findByRole("alert")).toHaveTextContent("Related memories could not be loaded");
    expect(screen.queryByText("No related memories found.")).toBeNull();
    vi.mocked(api.search).mockResolvedValue([]);
    await user.click(screen.getByRole("button", { name: "Retry loading" }));
    expect(await screen.findByText("No related memories found.")).toBeVisible();
  });

  it.each([archive, merge])("distinguishes a missing linked source from a fetch error for $action", async (item) => {
    vi.mocked(api.getPageSources).mockResolvedValue([{ source: { page_id: fixture.pages[0].id, memory_source_id: "deleted", linked_at: 0 }, memory: null }]);
    mount(item);
    const button = screen.getByRole("button", { name: item.action === "page_merge" ? "Merge notes" : "Archive" });
    await screen.findByText(/Some linked sources are unavailable/);
    expect(screen.queryByRole("button", { name: "Retry loading" })).toBeNull();
    if (item.action === "page_merge") expect(button).toBeDisabled();
    else expect(button).toBeEnabled();
  });

  it("shows actual repair choices without a generic approval", async () => {
    const user = userEvent.setup();
    mount({ ...archive, action: "lint_repair_review", payload: { action: "lint_repair_review", check_id: "source", occurrence_digest: "a".repeat(64), owner_binding_digest: "b".repeat(64), issue: "Quoted source changed", choices: ["Keep original", "Research source"], suggested_research_queries: [] } });
    expect(screen.getAllByText("Quoted source changed").some((element) => !element.closest("details"))).toBe(true);
    await user.click(screen.getByText(i18n.t("sourceRepair.showDetails"), { selector: "summary" }));
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
  });

  it.each([
    ["pages.semantic.provenance_adequacy", "provenanceHint"],
    ["pages.semantic.faithfulness", "faithfulnessHint"],
  ] as const)("shows the page title and localized human hint for %s", async (checkId, hintKey) => {
    const item: ReviewItem = {
      ...archive,
      id: `page-semantic-${checkId}`,
      action: "lint_repair_review",
      sourceIds: [fixture.pages[0].id],
      payload: {
        action: "lint_repair_review",
        check_id: checkId,
        occurrence_digest: "a".repeat(64),
        owner_binding_digest: "b".repeat(64),
        issue: "ReviewPageClaim: raw planner action",
        choices: ["confirm the Page claim", "revise or remove the Page claim"],
        suggested_research_queries: [],
      },
    };
    mount(item);

    expect(await screen.findByRole("heading", { name: fixture.pages[0].title })).toBeVisible();
    expect(screen.getByText(i18n.t(`sourceRepair.${hintKey}`))).toBeVisible();
    expect(screen.getByText(i18n.t("sourceRepair.unsupportedPending"))).toBeVisible();
    expect(screen.getByText(fixture.pages[0].content.replace(/\s+/g, " ").trim())).toBeVisible();
    expect(screen.getByRole("button", { name: "Keep current source" })).toBeVisible();
    const rawIssue = screen.getByText(/ReviewPageClaim/);
    expect(rawIssue.closest("details")).not.toBeNull();
    expect(rawIssue.closest("details")).not.toHaveAttribute("open");
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(api.getPage).toHaveBeenCalledWith(fixture.pages[0].id);
    expect(api.getMemoryDetail).not.toHaveBeenCalled();
  });

  it("does not treat mixed page and memory owners as a page target", async () => {
    const item: ReviewItem = {
      ...archive,
      id: "page-semantic-mixed",
      action: "lint_repair_review",
      sourceIds: [fixture.pages[0].id, fixture.memories[0].source_id],
      payload: {
        action: "lint_repair_review",
        check_id: "pages.semantic.provenance_adequacy",
        occurrence_digest: "a".repeat(64),
        owner_binding_digest: "b".repeat(64),
        issue: "ReviewPageClaim: raw planner action",
        choices: [],
        suggested_research_queries: [],
      },
    };
    mount(item);

    expect(await screen.findByText(i18n.t("sourceRepair.unsupportedPending"))).toBeVisible();
    expect(api.getPage).not.toHaveBeenCalled();
    expect(api.getMemoryDetail).not.toHaveBeenCalled();
    expect(screen.queryByText(fixture.pages[0].content)).toBeNull();
  });

  it("respects allowed actions for dismiss as well as approve", async () => {
    const { resolve, user } = mount({ ...archive, payload: { action: "page_keep_or_archive", page_id: fixture.pages[0].id, source_count: 0, allowed_actions: ["accept"] } });
    expect(screen.queryByRole("button", { name: "Keep note" })).toBeNull();
    await user.keyboard("d"); expect(resolve).not.toHaveBeenCalled();
  });

  it("links the archive page and the actual source instead of displaying only a claimed count", async () => {
    vi.mocked(api.getPageSources).mockResolvedValue([{ source: { memory_source_id: "memory-0" }, memory: fixture.memories[0] }] as api.PageSourceWithMemory[]);
    const { onOpenPage, onOpenMemory, user } = mount(archive);
    await user.click(await screen.findByRole("button", { name: /Fixture architecture/ }));
    expect(onOpenMemory).toHaveBeenCalledWith("memory-0");
    await user.click(screen.getByRole("button", { name: "Open note" }));
    expect(onOpenPage).toHaveBeenCalledWith(fixture.pages[0].id);
  });
});
