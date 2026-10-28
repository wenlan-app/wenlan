// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { getPage, type Page } from "../../lib/tauri";
import { useMilestones } from "./useMilestones";
import { FirstPageModal } from "./FirstPageModal";
import { useOnboardingOnStage } from "./onboardingStage";

const FIRST_CONCEPT_SHOWN_KEY = "onboarding:firstConceptShownCount";
const MAX_MODAL_SHOWS = 3;

export function FirstPageMilestone({ pages, onSelectPage }: {
  pages: readonly Page[];
  onSelectPage?: (pageId: string) => void;
}) {
  const { milestones, acknowledge } = useMilestones();
  // A new user's checklist lists the first page in its own step.
  const onStage = useOnboardingOnStage();

  const firstConceptMs = milestones.find(
    (m) => m.id === "first-concept" && m.acknowledged_at == null,
  );
  const firstConceptData = firstConceptMs?.payload as
    | { page_id?: string; title?: string }
    | undefined;
  const pageId = firstConceptData?.page_id;
  const providedPage = pageId
    ? pages.find((c) => c.id === pageId)
    : null;
  // Wiki's explicit browse snapshot does not poll. Resolve only the pending
  // milestone's target so a background first page can open its modal without
  // turning a passive onboarding read into an explicit browse intent.
  const targetPage = useQuery({
    queryKey: ["first-page-milestone", pageId],
    queryFn: () => getPage(pageId!),
    enabled: Boolean(pageId && !providedPage),
    retry: false,
    refetchInterval: (query) => query.state.data ? false : 2_000,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const firstConcept = providedPage ?? (pageId ? targetPage.data : null);

  // Track whether we've incremented shown_count for the current first-concept
  // modal instance, so StrictMode double-invokes don't double-count.
  const incrementedOnMountRef = useRef(false);
  useEffect(() => {
    if (!onStage || !firstConceptMs) return;
    acknowledge("first-concept");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onStage, firstConceptMs?.id]);

  useEffect(() => {
    if (onStage || !firstConcept || !firstConceptMs) return;
    if (incrementedOnMountRef.current) return;
    incrementedOnMountRef.current = true;
    const current = parseInt(
      localStorage.getItem(FIRST_CONCEPT_SHOWN_KEY) || "0",
      10,
    );
    const next = current + 1;
    localStorage.setItem(FIRST_CONCEPT_SHOWN_KEY, String(next));
    if (next > MAX_MODAL_SHOWS) {
      acknowledge("first-concept");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onStage, firstConcept?.id, firstConceptMs?.id]);

  const firstConceptShownCount = parseInt(
    localStorage.getItem(FIRST_CONCEPT_SHOWN_KEY) || "0",
    10,
  );
  const shouldShowFirstConceptModal =
    !onStage &&
    !!firstConcept &&
    !!firstConceptMs &&
    firstConceptShownCount <= MAX_MODAL_SHOWS;

  return (
    shouldShowFirstConceptModal && firstConcept ? (
        <FirstPageModal
          page={firstConcept}
          onOpen={(id) => {
            localStorage.removeItem(FIRST_CONCEPT_SHOWN_KEY);
            acknowledge("first-concept");
            onSelectPage?.(id);
          }}
          onDismiss={() => {
            localStorage.removeItem(FIRST_CONCEPT_SHOWN_KEY);
            acknowledge("first-concept");
          }}
        />
    ) : null
  );
}
