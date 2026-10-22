// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listRegisteredSources, type Page, type RegisteredSource } from "../../../lib/tauri";
import { useSteepOnDemand } from "../../../lib/steepMode";
import { AskAiHint, useHintDismissed } from "./AskAiHint";

function sourceName(source: RegisteredSource): string {
  const parts = source.path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? source.path;
}

/** The source to offer distilling for: one that brought memories in while no
 *  page has been written from memories yet. On demand only, since Automatic
 *  mode writes those pages itself. */
export function distillHintSource(
  sources: readonly RegisteredSource[],
  pages: readonly Page[],
  onDemand: boolean | undefined,
): RegisteredSource | null {
  if (onDemand !== true) return null;
  if (pages.some((page) => page.creation_kind === "distilled")) return null;
  return sources.find((source) => source.memory_count > 0) ?? null;
}

/** After a folder, Obsidian vault or Logseq graph comes in, say how to have
 *  the AI tool turn it into pages. */
export function DistillHint({ pages }: { readonly pages: readonly Page[] }) {
  const { t } = useTranslation();
  const onDemand = useSteepOnDemand();
  const sources = useQuery({
    queryKey: ["registeredSources"],
    queryFn: listRegisteredSources,
    refetchOnWindowFocus: true,
  });
  const source = distillHintSource(sources.data ?? [], pages, onDemand);
  const [dismissed, dismiss] = useHintDismissed(`distill:${source?.id ?? ""}`);
  if (!source || dismissed) return null;
  return (
    <AskAiHint
      task="distill"
      testId="ask-ai-hint-distill"
      lead={t("askAi.hints.distill", { name: sourceName(source) })}
      onDismiss={dismiss}
    />
  );
}
