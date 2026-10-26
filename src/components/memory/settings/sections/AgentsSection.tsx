// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  clipboardWrite,
  deleteAgent,
  detectMcpClients,
  removeRawMcpEntry,
  updateAgent,
  type AgentConnection,
  type McpClient,
} from "../../../../lib/tauri";
import { formatTimeAgo } from "../../../../lib/dateFormat";
import { describeSetupError } from "../../../../lib/setupErrors";
import { readingIsYes } from "../../../../lib/reading";
import {
  describeTrustLevel,
  familyDisplayName,
  OWN_IDENTITY,
  resolveAgentDisplayName,
  toolFamilyOf,
} from "../../../../lib/agents";
import { RemoteAccessPanel } from "../../RemoteAccessPanel";
import { Button, Card, ConfirmActionButton, SectionHeader, Select, Toggle } from "../primitives";
import ClientSetupList from "../../../connect/ClientSetupList";
import { connectClient, needsSkillsOnly } from "../../../connect/connectClient";
import { repairReasonOf } from "../../../connect/clientHealth";
import ConnectionListDisclosure from "../../../connect/ConnectionListDisclosure";
import {
  CONNECTION_ROW_LIMIT,
  familyOfClient,
  restartNameForClient,
  seenFamiliesOf,
  useLiveAgents,
} from "../../../connect/connectionState";

type AgentUpdate = { enabled?: boolean; trustLevel?: string };
type RowStatus = "repair" | "restart" | "connected";

interface FamilyRow {
  family: string;
  client?: McpClient;
  identities: AgentConnection[];
  status: RowStatus;
  memoryCount: number;
  latestLastSeen: number | null;
  anyEnabled: boolean;
  uniformTrust: string | null;
}

interface RowError {
  message: string;
  detail: string;
}

export default function AgentsSection({
  currentSpace,
}: {
  onSetupAgent?: () => void;
  currentSpace?: string;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [showAll, setShowAll] = useState(false);
  const [locallyAdded, setLocallyAdded] = useState<Set<string>>(() => new Set());
  const [disconnectedClientTypes, setDisconnectedClientTypes] = useState<Set<string>>(() => new Set());
  const [repairedClientTypes, setRepairedClientTypes] = useState<Set<string>>(() => new Set());
  const [busyFamily, setBusyFamily] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, RowError | null>>({});

  const { data: agents = [] } = useLiveAgents();
  const { data: mcpClients = [] } = useQuery({
    queryKey: ["mcp-clients"],
    queryFn: detectMcpClients,
    refetchOnWindowFocus: true,
    staleTime: 15_000,
  });

  const updateAgentMut = useMutation({
    mutationFn: ({ name, updates }: { name: string; updates: AgentUpdate }) => updateAgent(name, updates),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["agents"] }),
  });
  const deleteAgentMut = useMutation({
    mutationFn: deleteAgent,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["agents"] }),
  });
  const updateAll = (identities: AgentConnection[], updates: AgentUpdate) =>
    identities.forEach((identity) => updateAgentMut.mutate({ name: identity.name, updates }));

  const familyMap = new Map<string, AgentConnection[]>();
  for (const agent of agents) {
    const family = toolFamilyOf(agent);
    if (OWN_IDENTITY.test(family)) continue;
    const current = familyMap.get(family);
    if (current) current.push(agent);
    else familyMap.set(family, [agent]);
  }

  const clientByFamily = new Map<string, McpClient>();
  for (const client of mcpClients) {
    const family = familyOfClient(client.client_type);
    const current = clientByFamily.get(family);
    if (!current || readingIsYes(client.already_configured) || repairReasonOf(client) !== null) {
      clientByFamily.set(family, client);
    }
  }

  const disconnectedFamilies = new Set(
    [...disconnectedClientTypes].map((clientType) => familyOfClient(clientType)),
  );
  const seenFamilies = seenFamiliesOf(agents);
  const connectedFamilyKeys = new Set<string>();
  for (const family of familyMap.keys()) {
    if (!disconnectedFamilies.has(family)) connectedFamilyKeys.add(family);
  }
  for (const client of mcpClients) {
    if (disconnectedClientTypes.has(client.client_type)) continue;
    if (
      locallyAdded.has(client.client_type) ||
      readingIsYes(client.already_configured) ||
      repairReasonOf(client) !== null ||
      needsSkillsOnly(client)
    ) {
      connectedFamilyKeys.add(familyOfClient(client.client_type));
    }
  }

  const rows: FamilyRow[] = [...connectedFamilyKeys].map((family) => {
    const identities = familyMap.get(family) ?? [];
    const client = clientByFamily.get(family);
    const firstTrust = identities[0]?.trust_level ?? "unknown";
    const needsRepair = Boolean(
      client &&
      !repairedClientTypes.has(client.client_type) &&
      repairReasonOf(client) !== null,
    );
    const latestLastSeen = identities.reduce<number | null>(
      (latest, identity) => {
        const seenAt = identity.last_seen_at ?? (identity.memory_count > 0 ? identity.updated_at : null);
        return seenAt != null && (latest === null || seenAt > latest)
          ? seenAt
          : latest;
      },
      null,
    );
    return {
      family,
      client,
      identities,
      status: needsRepair ? "repair" : seenFamilies.has(family) ? "connected" : "restart",
      memoryCount: identities.reduce((count, identity) => count + identity.memory_count, 0),
      latestLastSeen,
      anyEnabled: identities.length === 0 || identities.some((identity) => identity.enabled),
      uniformTrust:
        identities.length > 0 && identities.every((identity) => identity.trust_level === firstTrust)
          ? firstTrust
          : null,
    };
  });

  rows.sort((left, right) => {
    const priority = { repair: 0, restart: 1, connected: 2 } satisfies Record<RowStatus, number>;
    return priority[left.status] - priority[right.status]
      || (right.latestLastSeen ?? 0) - (left.latestLastSeen ?? 0)
      || familyDisplayName(left.family).localeCompare(familyDisplayName(right.family));
  });

  const attentionRows = rows.filter((row) => row.status !== "connected");
  const recentRows = rows.filter((row) => row.status === "connected");
  const visibleRows = showAll
    ? rows
    : [
        ...attentionRows,
        ...recentRows.slice(0, Math.max(0, CONNECTION_ROW_LIMIT - attentionRows.length)),
      ];
  const hiddenCount = rows.length - visibleRows.length;

  const repair = async (row: FamilyRow) => {
    if (!row.client) return;
    setBusyFamily(row.family);
    setRowErrors((previous) => ({ ...previous, [row.family]: null }));
    try {
      await connectClient(row.client, "repair");
      setRepairedClientTypes((previous) => new Set(previous).add(row.client!.client_type));
      void queryClient.invalidateQueries({ queryKey: ["mcp-clients"] });
    } catch (error) {
      const described = describeSetupError(error);
      setRowErrors((previous) => ({
        ...previous,
        [row.family]: {
          message: t("connectMatrix.repairFailed", { name: familyDisplayName(row.family) }),
          detail: described.detail,
        },
      }));
    } finally {
      setBusyFamily(null);
    }
  };

  const disconnect = async (row: FamilyRow) => {
    if (!row.client) {
      updateAll(row.identities, { enabled: !row.anyEnabled });
      return;
    }
    setBusyFamily(row.family);
    setRowErrors((previous) => ({ ...previous, [row.family]: null }));
    try {
      await removeRawMcpEntry(row.client.client_type);
      setDisconnectedClientTypes((previous) => new Set(previous).add(row.client!.client_type));
      void queryClient.invalidateQueries({ queryKey: ["mcp-clients"] });
    } catch (error) {
      const described = describeSetupError(error);
      setRowErrors((previous) => ({
        ...previous,
        [row.family]: {
          message: t("connectMatrix.disconnectFailed", { name: familyDisplayName(row.family) }),
          detail: described.detail,
        },
      }));
    } finally {
      setBusyFamily(null);
    }
  };

  const onClientConnected = (clientType: string) => {
    setLocallyAdded((previous) => new Set(previous).add(clientType));
    setDisconnectedClientTypes((previous) => {
      const next = new Set(previous);
      next.delete(clientType);
      return next;
    });
  };

  return (
    <>
      <section data-connection-section="connected" className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SectionHeader label={t("settings.agents.connectedAgents")} />
        <Card padding="none">
          <div className="flex flex-col gap-1 p-2">
            {visibleRows.length === 0 ? (
              <p className="px-3 py-4 text-[length:var(--mem-text-sm)] text-[var(--mem-text-tertiary)]">
                {t("settings.agents.noAgents")}
              </p>
            ) : visibleRows.map((row) => (
              <ToolFamilyRow
                key={row.family}
                row={row}
                agents={agents}
                expanded={Boolean(expanded[row.family])}
                busy={busyFamily === row.family}
                error={rowErrors[row.family]}
                onToggleExpand={() => setExpanded((previous) => ({
                  ...previous,
                  [row.family]: !previous[row.family],
                }))}
                onToggle={() => void disconnect(row)}
                onRepair={() => void repair(row)}
                updateAll={updateAll}
                onDelete={(name) => deleteAgentMut.mutate(name)}
              />
            ))}
            {(hiddenCount > 0 || showAll) && rows.length > CONNECTION_ROW_LIMIT && (
              <ConnectionListDisclosure
                expanded={showAll}
                hiddenCount={hiddenCount}
                onToggle={() => setShowAll((value) => !value)}
              />
            )}
          </div>
        </Card>
      </section>

      <ClientSetupList
        connectedFamilies={connectedFamilyKeys}
        disconnectedClientTypes={disconnectedClientTypes}
        onAddedChange={setLocallyAdded}
        onClientConnected={onClientConnected}
      />

      <section className="mem-fade-up" style={{ animationDelay: "60ms" }}>
        <RemoteAccessPanel currentSpace={currentSpace} />
      </section>
    </>
  );
}

function ToolFamilyRow({
  row,
  agents,
  expanded,
  busy,
  error,
  onToggleExpand,
  onToggle,
  onRepair,
  updateAll,
  onDelete,
}: {
  row: FamilyRow;
  agents: AgentConnection[];
  expanded: boolean;
  busy: boolean;
  error?: RowError | null;
  onToggleExpand: () => void;
  onToggle: () => void;
  onRepair: () => void;
  updateAll: (identities: AgentConnection[], updates: AgentUpdate) => void;
  onDelete: (name: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const displayName = familyDisplayName(row.family);
  const subtitleKey = row.client?.client_type === "codex_cli"
    ? "connectMatrix.subtitleCodex"
    : row.client?.client_type === "kimi_desktop"
      ? "connectMatrix.subtitleKimiDesktop"
      : row.client?.client_type === "kimi_code"
        ? "connectMatrix.subtitleKimiCode"
        : null;
  const trust = row.uniformTrust ? describeTrustLevel(row.uniformTrust).level : null;

  return (
    <div
      data-connection-row
      data-family={row.family}
      data-status={row.status}
      className="rounded-[var(--mem-radius-md)] px-3 py-2.5 hover:bg-[var(--mem-hover)]"
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-[length:var(--mem-text-md)] font-medium text-[var(--mem-text)]">
              {displayName}
            </span>
            <ConnectionStatus row={row} locale={i18n.language} />
            {row.status === "repair" && (
              <Button
                type="button"
                variant="secondary"
                size="sm"
                style={{ height: "30px", paddingInline: "12px", borderRadius: "8px", fontSize: "13px" }}
                onClick={onRepair}
                disabled={busy}
              >
                {t("connectMatrix.repair")}
              </Button>
            )}
          </div>
          {subtitleKey && (
            <p className="mt-0.5 truncate text-[length:var(--mem-text-xs)] text-[var(--mem-text-tertiary)]">
              {t(subtitleKey)}
            </p>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Toggle
            enabled={row.anyEnabled}
            valueUnknown={busy}
            unknownState="saving"
            onToggle={onToggle}
            aria-label={t("connectMatrix.disconnectTool", { name: displayName })}
          />
          <button
            type="button"
            onClick={onToggleExpand}
            aria-expanded={expanded}
            aria-label={t("connectMatrix.detailsLabel", { name: displayName })}
            className="flex h-7 w-7 items-center justify-center rounded-[var(--mem-radius-sm)] text-[var(--mem-text-tertiary)] hover:text-[var(--mem-text)] focus-visible:outline-2 focus-visible:outline-[var(--mem-focus-ring)] focus-visible:outline-offset-2"
          >
            <svg
              className="h-3.5 w-3.5 transition-transform"
              style={{ transform: expanded ? "rotate(90deg)" : "rotate(0deg)" }}
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
            </svg>
          </button>
        </div>
      </div>

      {error && <InlineError error={error} />}

      {expanded && (
        <div className="mt-3 flex flex-col gap-3 pl-1">
          <div className="flex flex-wrap items-center gap-3">
            <label className="text-[length:var(--mem-text-xs)] text-[var(--mem-text-tertiary)]">
              <span className="mr-2">{t("settings.agents.trustLevels")}</span>
              <Select
                size="sm"
                value={row.identities.length === 0 ? "unknown" : trust ?? ""}
                disabled={row.identities.length === 0}
                aria-label={`${t("settings.agents.trustLevels")}: ${displayName}`}
                onChange={(event) => {
                  if (event.target.value) updateAll(row.identities, { trustLevel: event.target.value });
                }}
              >
                {row.identities.length > 0 && trust === null && (
                  <option value="" disabled>{t("connections.mixed")}</option>
                )}
                <option value="full">{t("settings.agents.trust.full")}</option>
                <option value="review">{t("settings.agents.trust.review")}</option>
                <option value="unknown">{t("settings.agents.trust.unknown")}</option>
              </Select>
            </label>
            <span className="text-[length:var(--mem-text-xs)] text-[var(--mem-text-tertiary)]">
              {t("settings.agents.memories", { count: row.memoryCount })}
            </span>
          </div>
          {row.client?.config_path && (
            <code
              className="truncate text-[length:var(--mem-text-2xs)] text-[var(--mem-text-tertiary)]"
              style={{ fontFamily: "var(--mem-font-mono)" }}
            >
              {row.client.config_path}
            </code>
          )}
          {row.identities.length > 0 && (
            <ul className="flex flex-col gap-2" aria-label={t("connections.identities", { count: row.identities.length })}>
              {row.identities.map((identity) => {
                const identityName = resolveAgentDisplayName(identity.name, agents);
                return (
                  <li key={identity.id} className="flex items-center justify-between gap-3">
                    <span className="truncate text-[length:var(--mem-text-xs)] text-[var(--mem-text-secondary)]">
                      {identityName}
                    </span>
                    <ConfirmActionButton
                      variant="ghost"
                      size="sm"
                      aria-label={t("settings.agents.deleteAgent", { name: identityName })}
                      confirmLabel={t("settings.agents.confirm")}
                      cancelLabel={t("settings.agents.cancel")}
                      onConfirm={() => onDelete(identity.name)}
                    >
                      {t("settings.agents.deleteIdentity")}
                    </ConfirmActionButton>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function ConnectionStatus({ row, locale }: { row: FamilyRow; locale: string }) {
  const { t } = useTranslation();
  if (row.status === "repair") {
    return (
      <span className="inline-flex shrink-0 items-center gap-1.5 text-[length:var(--mem-text-xs)] text-[var(--mem-status-warning-text)]">
        <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-[var(--mem-accent-warm)]" />
        {t("connectMatrix.needsRepair")}
      </span>
    );
  }
  if (row.status === "restart") {
    const displayName = familyDisplayName(row.family);
    const restartName = restartNameForClient(
      row.client?.client_type ?? row.family,
      displayName,
      t,
    );
    return (
      <span className="inline-flex shrink-0 items-center gap-1.5 text-[length:var(--mem-text-xs)] text-[var(--mem-status-warning-text)]">
        <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full border border-[var(--mem-accent-amber)]" />
        {t("connectMatrix.restartToFinish", { name: restartName })}
      </span>
    );
  }
  const timestamp = row.latestLastSeen ?? 0;
  const time = formatTimeAgo(timestamp, locale.startsWith("en") ? undefined : locale);
  return (
    <span className="inline-flex shrink-0 items-center gap-1.5 text-[length:var(--mem-text-xs)] text-[var(--mem-text-secondary)]">
      <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-[var(--mem-accent-sage)]" />
      {t("connectMatrix.connectedUsed", { time })}
    </span>
  );
}

function InlineError({ error }: { error: RowError }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div role="alert" className="mt-2 flex flex-wrap items-center gap-2 text-[length:var(--mem-text-xs)] text-[var(--mem-status-danger-text)]">
      <span>{error.message}</span>
      {error.detail && (
        <button
          type="button"
          className="rounded-[var(--mem-radius-sm)] text-[var(--mem-text-secondary)] hover:text-[var(--mem-text)] focus-visible:outline-2 focus-visible:outline-[var(--mem-focus-ring)] focus-visible:outline-offset-2"
          onClick={() => void clipboardWrite(error.detail)
            .then(() => setCopied(true))
            .catch(() => setCopied(false))}
        >
          {copied ? t("connectMatrix.copied") : t("connectMatrix.copyDetails")}
        </button>
      )}
    </div>
  );
}
