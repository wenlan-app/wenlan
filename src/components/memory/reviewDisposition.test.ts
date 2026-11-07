// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { ReviewItem } from "./useReviewQueue";
import { isActionableReviewItem } from "./reviewDisposition";
import { reviewApproveBlocked, reviewDismissBlocked } from "./ReviewDialog";

const revision: ReviewItem = {
  kind: "revision",
  id: "memory-1",
  targetSourceId: "memory-1",
  targetKind: "memory",
  revisionSourceId: "revision-1",
  content: "Updated content",
  agent: null,
  timestampMs: null,
};

describe("isActionableReviewItem", () => {
  it("accepts decision items only when the existing dialog contract allows approval", () => {
    expect(isActionableReviewItem(revision)).toBe(true);
    expect(isActionableReviewItem({ ...revision, targetSourceId: " " })).toBe(false);
    expect(isActionableReviewItem({ ...revision, id: "example:memory-1" })).toBe(false);
  });

  it("admits only valid scoped research disputes to their dedicated decision flow", () => {
    const digest = "a".repeat(64);
    const item: ReviewItem = {
      kind: "refinement", id: "dispute-1", action: "research_dispute", sourceIds: ["paper-a", "paper-b"], confidence: 0, timestampMs: null,
      payload: { action: "research_dispute", space: "Research", payload_digest: digest, dispute: {
        space_id: "space-1", kind: "incomparable_conditions", conditions: "Different methods", reason: "Compare findings", evidence: [
          { source: "file", source_id: "paper-a", source_digest: digest, quote: "First finding" },
          { source: "file", source_id: "paper-b", source_digest: digest, quote: "Second finding" },
        ],
      } },
    };
    expect(isActionableReviewItem(item)).toBe(true);
    expect(reviewApproveBlocked(item)).toBe(true);
    expect(reviewDismissBlocked(item)).toBe(true);
    expect(isActionableReviewItem({ ...item, id: "  " })).toBe(false);
    if (item.payload?.action !== "research_dispute") throw new Error("fixture must be a dispute");
    expect(isActionableReviewItem({ ...item, payload: { ...item.payload, space: " " } })).toBe(false);
    expect(isActionableReviewItem({ ...item, payload: null })).toBe(false);
  });

  it("keeps captures and background discovery outside the decision queue", () => {
    expect(isActionableReviewItem({
      kind: "capture", id: "capture-1", title: "A note", snippet: null, timestampMs: null,
    })).toBe(false);
    expect(isActionableReviewItem({
      kind: "page_candidate", id: "page-1", title: "New page", cluster: {} as never, timestampMs: null,
    })).toBe(false);
    expect(isActionableReviewItem({
      kind: "topic", id: "topic-1", label: "Topic", count: 2, timestampMs: null,
    })).toBe(false);
  });

  it("admits lint repair only when its dedicated flow payload action matches", () => {
    const matching: ReviewItem = {
      kind: "refinement",
      id: "repair-1",
      action: "lint_repair_review",
      sourceIds: ["memory-1"],
      payload: {
        action: "lint_repair_review",
        check_id: "check-1",
        occurrence_digest: "occurrence",
        owner_binding_digest: "owner",
        issue: "Review this source",
        choices: ["Keep", "Remove"],
        suggested_research_queries: [],
      },
      confidence: 1,
      timestampMs: null,
    };
    expect(isActionableReviewItem(matching)).toBe(true);
    expect(isActionableReviewItem({ ...matching, payload: { action: "detect_contradiction" } as never })).toBe(false);
  });

  it("leaves stale page refreshes actionable and rejects unsupported proposals", () => {
    expect(isActionableReviewItem({
      kind: "stale_page", id: "page-1", title: "Page", summary: null, sourcesUpdated: 1, timestampMs: null,
    })).toBe(true);
    expect(isActionableReviewItem({
      kind: "refinement",
      id: "cross-space",
      action: "cross_space_discovery",
      sourceIds: ["memory-1"],
      payload: { action: "cross_space_discovery", memory_count: 1, spaces: ["Other"] },
      confidence: 0.8,
      timestampMs: null,
    })).toBe(false);
  });
});
