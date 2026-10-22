// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getActiveImportBatches } from "../../../lib/tauri";
import { ImportDetailPanel, ImportStatusPill } from "../ImportPhases";
import { useSteepOnDemand } from "../../../lib/steepMode";

/** Import progress belongs beside the sources being brought into the library. */
export function SourceImportProgress() {
  const [expanded, setExpanded] = useState(false);
  const { data } = useQuery({
    queryKey: ["active-import-batches"],
    queryFn: getActiveImportBatches,
    refetchInterval: 10_000,
  });
  const onDemand = useSteepOnDemand() === true;
  const batches = data?.batches ?? [];
  const hasImports = batches.length > 0;
  useEffect(() => {
    if (!hasImports) setExpanded(false);
  }, [hasImports]);
  if (!hasImports) return null;
  return (
    <div className="source-import-progress">
      {expanded ? (
        <ImportDetailPanel batches={batches} onBack={() => setExpanded(false)} onDemand={onDemand} />
      ) : (
        <ImportStatusPill batches={batches} onOpen={() => setExpanded(true)} />
      )}
    </div>
  );
}
