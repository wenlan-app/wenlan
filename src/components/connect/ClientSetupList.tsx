// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { detectMcpClients, type McpClient, type UndeterminedInput } from "../../lib/tauri";
import { familyDisplayName } from "../../lib/agents";
import { readingIsYes } from "../../lib/reading";
import { describeSetupError, type DescribedSetupError } from "../../lib/setupErrors";
import { connectClient, needsSkillsOnly } from "./connectClient";
import { unreadPluginWriteRisk } from "./setupRisk";
import ClientRow, { clientRowDescId } from "./ClientRow";
import ConnectionListDisclosure from "./ConnectionListDisclosure";
import { CONNECTION_ROW_LIMIT, familyOfClient } from "./connectionState";
import { isConfigOnly, repairReasonOf } from "./clientHealth";
import { Button, Card, SectionHeader, Toggle } from "../memory/settings/primitives";

interface SuccessfulWarning {
  message: string;
  details: string[];
}

/** Detected tools not represented in Connected. The parent owns the family
 * merge; this component owns the existing setup write path and reports a
 * successful write immediately so the row can move before the next scan. */
export default function ClientSetupList({
  connectedFamilies = new Set<string>(),
  disconnectedClientTypes = new Set<string>(),
  onAddedChange,
  onClientConnected,
  hideWhenComplete = false,
}: {
  connectedFamilies?: Set<string>;
  /** Kept for callers outside Settings; live status now renders in Connected. */
  seenFamilies?: Set<string>;
  disconnectedClientTypes?: Set<string>;
  onAddedChange?: (clientTypes: Set<string>) => void;
  onClientConnected?: (clientType: string) => void;
  hideWhenComplete?: boolean;
} = {}) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [connectingAll, setConnectingAll] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [added, setAdded] = useState<Record<string, true>>({});
  const [errors, setErrors] = useState<Record<string, DescribedSetupError | null>>({});
  const [warnings, setWarnings] = useState<Record<string, SuccessfulWarning | null>>({});

  useEffect(() => {
    onAddedChange?.(new Set(Object.keys(added)));
  }, [added, onAddedChange]);

  const { data: clients, isError, refetch } = useQuery({
    queryKey: ["mcp-clients"],
    queryFn: detectMcpClients,
  });

  const availableRows = useMemo(
    () =>
      (clients ?? []).filter((client) => {
        const family = familyOfClient(client.client_type);
        const forcedAvailable = disconnectedClientTypes.has(client.client_type);
        if (forcedAvailable) return true;
        if (added[client.client_type] || connectedFamilies.has(family)) return false;
        if (client.detected.kind === "no") return false;
        if (repairReasonOf(client) !== null) return false;
        return !readingIsYes(client.already_configured) && !needsSkillsOnly(client);
      }),
    [added, clients, connectedFamilies, disconnectedClientTypes],
  );

  const notInstalledNames = (clients ?? [])
    .filter((client) => client.detected.kind === "no")
    .map((client) => familyDisplayName(familyOfClient(client.client_type)));
  // Chinese ListFormat glues "和" to Latin names ("Kiro和Kimi Code"); the 、 list reads cleanly.
  const notInstalledList = notInstalledNames.length === 0
    ? ""
    : i18n.language.startsWith("zh")
      ? notInstalledNames.join("、")
      : new Intl.ListFormat(i18n.language, { style: "long", type: "conjunction" }).format(notInstalledNames);

  const setUp = async (client: McpClient) => {
    const clientType = client.client_type;
    setBusy(clientType);
    setErrors((previous) => ({ ...previous, [clientType]: null }));
    setWarnings((previous) => ({ ...previous, [clientType]: null }));
    try {
      const undetermined = await connectClient(client, "setup");
      if (undetermined.length > 0) {
        setWarnings((previous) => ({
          ...previous,
          [clientType]: warningForUndetermined(undetermined, t),
        }));
      }
      setAdded((previous) => ({ ...previous, [clientType]: true }));
      onClientConnected?.(clientType);
      void queryClient.invalidateQueries({ queryKey: ["mcp-clients"] });
      void queryClient.invalidateQueries({ queryKey: ["agents"] });
    } catch (error) {
      setErrors((previous) => ({ ...previous, [clientType]: describeSetupError(error) }));
    } finally {
      setBusy(null);
    }
  };

  const connectAllTargets = availableRows.filter(
    (client) =>
      readingIsYes(client.detected) &&
      !isConfigOnly(client) &&
      unreadPluginWriteRisk(client) === null,
  );
  const connectAll = async () => {
    setConnectingAll(true);
    try {
      // Several tools share skills directories, so keep the existing serial
      // writes and their per-write backups.
      for (const client of connectAllTargets) await setUp(client);
    } finally {
      setConnectingAll(false);
    }
  };

  const firstRun = connectedFamilies.size === 0;
  const visibleRows = firstRun || showAll
    ? availableRows
    : availableRows.slice(0, CONNECTION_ROW_LIMIT);
  const hiddenCount = availableRows.length - visibleRows.length;

  const heading = (
    <SectionHeader
      label={t("connectMatrix.notConnectedTitle")}
      action={connectAllTargets.length > 0 ? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => void connectAll()}
          disabled={connectingAll || busy !== null}
        >
          {connectingAll ? t("connectMatrix.settingUp") : t("connectMatrix.connectAll")}
        </Button>
      ) : undefined}
    />
  );

  if (clients === undefined && !isError) {
    return (
      <section data-connection-section="available" className="mem-fade-up" style={{ animationDelay: "30ms" }}>
        {heading}
        <div role="status" className="text-[length:var(--mem-text-sm)] text-[var(--mem-text-tertiary)]">
          {t("connectMatrix.loadingTools")}
        </div>
      </section>
    );
  }

  if (isError) {
    return (
      <section data-connection-section="available" className="mem-fade-up" style={{ animationDelay: "30ms" }}>
        {heading}
        <div className="flex items-center gap-2 flex-wrap">
          <p role="alert" className="text-[length:var(--mem-text-sm)] text-[var(--mem-status-danger-text)]">
            {t("connectMatrix.loadToolsError")}
          </p>
          <Button type="button" variant="secondary" size="sm" onClick={() => void refetch()}>
            {t("connectMatrix.retryTools")}
          </Button>
        </div>
      </section>
    );
  }

  if (hideWhenComplete && availableRows.length === 0 && !notInstalledList) return null;

  return (
    <section data-connection-section="available" className="mem-fade-up" style={{ animationDelay: "30ms" }}>
      {heading}
      {availableRows.length > 0 ? (
        <Card padding="none">
          <div className="flex flex-col gap-1 p-2">
            {visibleRows.map((client) => {
              const clientType = client.client_type;
              const family = familyOfClient(clientType);
              const displayName = familyDisplayName(family);
              const duplicateRisk = unreadPluginWriteRisk(client);
              const failure = errors[clientType];
              const successfulWarning = warnings[clientType];
              return (
                <div key={clientType} data-family={family}>
                  <ClientRow
                    client={client}
                    bare
                    configured={{ kind: "no" }}
                    error={failure ? t("connectMatrix.addFailed", { name: displayName }) : null}
                    errorDetail={failure?.detail}
                    technicalDetails={[
                      duplicateRisk,
                      ...(successfulWarning?.details ?? []),
                    ]}
                    warning={
                      duplicateRisk
                        ? t("connectMatrix.pluginStateUnknownBeforeWrite")
                        : successfulWarning?.message ?? null
                    }
                    trailing={(
                      <div className="flex items-center gap-2">
                        <Toggle
                          enabled={false}
                          valueUnknown={busy === clientType || connectingAll}
                          unknownState="saving"
                          onToggle={() => void setUp(client)}
                          aria-label={t("connectMatrix.connectTool", { name: displayName })}
                          aria-describedby={clientRowDescId(clientType)}
                        />
                        <span
                          aria-hidden="true"
                          data-connection-chevron-slot
                          className="block h-7 w-7 shrink-0"
                        />
                      </div>
                    )}
                  />
                </div>
              );
            })}
            {(hiddenCount > 0 || showAll) && !firstRun && (
              <ConnectionListDisclosure
                expanded={showAll}
                hiddenCount={hiddenCount}
                onToggle={() => setShowAll((value) => !value)}
              />
            )}
          </div>
        </Card>
      ) : clients?.length === 0 ? (
        <span role="status" className="text-[length:var(--mem-text-sm)] text-[var(--mem-text-tertiary)]">
          {t("connectMatrix.noToolsAvailable")}
        </span>
      ) : null}
      {notInstalledList && (
        <p className="mt-3 px-1 text-[length:var(--mem-text-xs)] text-[var(--mem-text-tertiary)]">
          {t("connectMatrix.alsoWorksWith", { names: notInstalledList })}
        </p>
      )}
    </section>
  );
}

function warningForUndetermined(
  inputs: UndeterminedInput[],
  t: TFunction,
): SuccessfulWarning {
  return {
    message: inputs
      .map((input) => t("connectMatrix.wroteWithUndeterminedInput", {
        input: input.input,
        blocked: input.blocked,
      }))
      .join(" "),
    details: inputs.map((input) => input.error),
  };
}
