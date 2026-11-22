// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery, type QueryClient } from "@tanstack/react-query";
import { getRulesSetupStatus, getRulesSetupReview, getRulesSkillUpdates, type RulesSetupStatus } from "../../../lib/tauri";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { useScope } from "../team/scope";

const STATUS = ["rules-setup-status"] as const;
const REVIEW = ["rules-setup-review"] as const;
const SKILL_UPDATES = ["rules-setup-skill-updates"] as const;
export const RULES_DELIVERY_QUERY_KEY = ["rules-delivery"] as const;

export function useRulesSetup(enabled = true): RulesSetupStatus | undefined {
  const example = useExampleWikiActive();
  const { team } = useScope();
  const status = useQuery({
    queryKey: STATUS,
    queryFn: () => getRulesSetupStatus(),
    enabled: enabled && !__WENLAN_REVIEW__ && !example && !team,
    staleTime: 15_000,
    refetchInterval: ({ state: { data } }) => data?.enabled && data.adopted ? 15_000 : false,
    refetchOnWindowFocus: "always",
    retry: false,
  });
  if (!enabled || __WENLAN_REVIEW__ || example || team || !status.isSuccess) return undefined;
  return status.data?.enabled ? status.data : undefined;
}

export function useRulesSetupReview(enabled: boolean) {
  return useQuery({
    queryKey: REVIEW,
    queryFn: () => getRulesSetupReview(),
    enabled,
    staleTime: 15_000,
    refetchInterval: enabled ? 15_000 : false,
    retry: false,
  });
}

export function useRulesSkillUpdates(enabled: boolean): ReadonlySet<string> | undefined {
  const updates = useQuery({
    queryKey: SKILL_UPDATES,
    queryFn: () => getRulesSkillUpdates(),
    enabled,
    staleTime: 5 * 60_000,
    retry: false,
    select: (rows) => new Set(rows.filter((row) => row.status.state === "available").map((row) => row.name)),
  });
  return enabled && updates.isSuccess ? updates.data : undefined;
}

export async function refreshRulesSetup(client: QueryClient, refreshSkills = false) {
  await Promise.all([
    client.invalidateQueries({ queryKey: STATUS }),
    client.invalidateQueries({ queryKey: REVIEW }),
    client.invalidateQueries({ queryKey: RULES_DELIVERY_QUERY_KEY }),
    ...(refreshSkills ? [client.invalidateQueries({ queryKey: SKILL_UPDATES })] : []),
  ]);
}
