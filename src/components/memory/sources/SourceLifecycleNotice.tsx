// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getSourceLifecycle, sourceLifecycleKey } from "../../../lib/sourceLifecycleApi";

export function SourceLifecycleNotice({ space, source, sourceId }: { space?: string; source: string; sourceId: string }) {
  const { t } = useTranslation();
  const status = useQuery({ queryKey: sourceLifecycleKey(space, source, sourceId), queryFn: () => getSourceLifecycle(space!, source, sourceId), enabled: !!space && ["file", "webpage"].includes(source), retry: false });
  if (!status.data || status.data.record.state === "active") return null;
  return <p role="status" className="source-lifecycle-notice">{t(`sourceLifecycle.${status.data.record.state}`)}: {status.data.record.reason}</p>;
}
