// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { detectMcpClients, removeRawMcpEntry, type McpClient } from "../../lib/tauri";
import { readingIsNo, readingIsYes } from "../../lib/reading";
import { describeSetupError, setupErrorHeadingKey, type DescribedSetupError } from "../../lib/setupErrors";
import { connectClient, isSkillsClient, needsSkillsOnly } from "./connectClient";
import { isPluginClient } from "./pluginClients";
import { unreadPluginWriteRisk } from "./setupRisk";
import ClientRow, { clientRowDescId } from "./ClientRow";
import { familyOfClient, type ConnectionStatus } from "./connectionState";
import { entryUnreadable, isConfigOnly, repairReasonOf } from "./clientHealth";
import { Button, Card, SectionHeader, Toggle } from "../memory/settings/primitives";

/** Apps & CLIs group. Every installed tool that is not connected yet gets one
 *  row with a switch; turning it on is the one "Set up", and `connectClient`
 *  decides what that does: Claude Code and Codex get the Wenlan plugin (which
 *  registers the MCP server itself), everyone else gets an MCP config write,
 *  plus Wenlan's skills for a tool that reads them from its own folder.
 *  Writing a config for a plugin client would register Wenlan twice, so this
 *  surface obeys the same invariant the wizard does. With two or more such
 *  tools, "Connect all" sets them all up in one press. Tools measured as not
 *  installed get no row; they are named once on an "Also works with" line.
 *
 *  `connectedFamilies` (the tool families the roster above already shows)
 *  is the single source of truth for what to hide here: a client whose family
 *  already has an identity is represented above, so re-listing it — even when
 *  its own config file looks unconfigured — is the duplication the user
 *  vetoed. The exception is a tool connected before it had skills: its row
 *  stays with an "Add skills" button.
 *
 *  `seenFamilies` is the narrower set whose identities have actually called
 *  Wenlan. It decides Added versus Connected. A tool the user adds from this
 *  list stays in place, saying "Added. Restart it to finish", and flips to
 *  Connected when its family first shows up in `seenFamilies`, instead of
 *  vanishing the moment its config is written. `onAddedChange` tells the
 *  roster above which tools those are, so it does not list them a second
 *  time as pending. */
export default function ClientSetupList({
  connectedFamilies,
  seenFamilies,
  hideWhenComplete = false,
  onAddedChange,
}: {
  connectedFamilies?: Set<string>;
  seenFamilies?: Set<string>;
  onAddedChange?: (clientTypes: Set<string>) => void;
  /** Hide a completed Add a tool section in compact Settings surfaces. */
  hideWhenComplete?: boolean;
} = {}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [connectingAll, setConnectingAll] = useState(false);
  const [errors, setErrors] = useState<
    Record<string, (DescribedSetupError & { mode: "setup" | "repair" }) | null>
  >({});
  // Clients this list added since it mounted. They keep their row.
  const [added, setAdded] = useState<Record<string, true>>({});
  // The subset of those that were repaired rather than set up, which changes
  // what the row says next ("Repaired. Restart X to finish.").
  const [repaired, setRepaired] = useState<Record<string, true>>({});
  useEffect(() => {
    onAddedChange?.(new Set(Object.keys(added)));
  }, [added, onAddedChange]);
  const seen = seenFamilies ?? connectedFamilies;
  const statusOf = (clientType: string): ConnectionStatus =>
    seen?.has(familyOfClient(clientType)) ? "connected" : "added";
  // Non-fatal notes from a write that SUCCEEDED. Round 6, D3's boundary
  // defect: `writeMcpConfig` now resolves with the resolver inputs it could not
  // determine, and a success that skipped candidates it never built must not
  // render as a plain success.
  const [warnings, setWarnings] = useState<Record<string, string>>({});

  const { data: clients, isError, refetch } = useQuery({ queryKey: ["mcp-clients"], queryFn: detectMcpClients });
  // Hide a client that is already configured (nothing left to do) OR whose
  // tool family is already connected in the roster above.
  // `!readingIsYes`, not `!configured`: a client whose config could not be
  // READ is still actionable — hiding it would be "nothing left to do here"
  // stated from a look that failed. Its row carries the unknown chip.
  //
  // A client whose own entry would not start is kept even when it is
  // configured or its family is connected: "nothing left to do" is false while
  // the entry is broken, and this row carries the Repair button.
  //
  // An entry that could not be read is kept too, for the same reason an
  // unreadable `already_configured` is: hiding it would state "nothing left to
  // do" from a look that failed. A family the roster already shows is the
  // evidence that the tool works, so that wins over a failed look.
  const actionable = (clients ?? []).filter((client) => {
    const familyConnected = connectedFamilies?.has(familyOfClient(client.client_type)) ?? false;
    return (
      added[client.client_type] ||
      repairReasonOf(client) !== null ||
      // Connected before Wenlan shipped skills for it: one step left.
      (needsSkillsOnly(client) && !readingIsNo(client.detected)) ||
      (entryUnreadable(client) && !familyConnected) ||
      (!readingIsYes(client.already_configured) && !familyConnected)
    );
  });
  const section = (content: ReactNode) => (
    <section className="mem-fade-up" style={{ animationDelay: "30ms" }}>
      {content}
    </section>
  );
  const title = <SectionHeader label={t("connectMatrix.addToolTitle")} />;

  if (clients === undefined && !isError) {
    return section(
      <>
        {title}
        <div role="status" style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-tertiary)" }}>
          {t("connectMatrix.loadingTools")}
        </div>
      </>,
    );
  }

  if (isError) {
    return section(
      <>
        {title}
        <div className="flex items-center gap-2 flex-wrap">
          <p role="alert" style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-status-danger-text)" }}>
            {t("connectMatrix.loadToolsError")}
          </p>
          <Button type="button" variant="secondary" size="sm" onClick={() => void refetch()}>
            {t("connectMatrix.retryTools")}
          </Button>
        </div>
      </>,
    );
  }

  if (clients?.length === 0) {
    return section(
      <>
        {title}
        <span role="status" style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-tertiary)" }}>
          {t("connectMatrix.noToolsAvailable")}
        </span>
      </>,
    );
  }

  /** `repair` rewrites the entry the client already has through the same
   *  `write_mcp_config` Set up uses, for every client including the plugin
   *  ones: the broken thing IS the raw entry, and rewriting it in place adds
   *  no second registration. */
  const setUp = async (client: McpClient, mode: "setup" | "repair" = "setup") => {
    const clientType = client.client_type;
    setBusy(clientType);
    setErrors((prev) => ({ ...prev, [clientType]: null }));
    setWarnings((prev) => ({ ...prev, [clientType]: "" }));
    try {
      const undetermined = await connectClient(client, mode);
      // An empty array is a measurement: every resolver input was read.
      // A non-empty one is the round-5 D4 fact arriving at the pixel — the
      // entry was written by a search that never built some of its
      // candidates, and saying nothing would make that identical to a search
      // that read them all.
      if (undetermined.length > 0) {
        setWarnings((prev) => ({
          ...prev,
          [clientType]: undetermined
            .map((u) => t("connectMatrix.wroteWithUndeterminedInput", { ...u }))
            .join(" "),
        }));
      }
      setAdded((prev) => ({ ...prev, [clientType]: true }));
      if (mode === "repair") setRepaired((prev) => ({ ...prev, [clientType]: true }));
      queryClient.invalidateQueries({ queryKey: ["mcp-clients"] });
      // The tool may already be running and reach Wenlan on its own; look now
      // rather than at the next poll.
      queryClient.invalidateQueries({ queryKey: ["agents"] });
    } catch (err) {
      setErrors((prev) => ({
        ...prev,
        [clientType]: { ...describeSetupError(err), mode },
      }));
    } finally {
      setBusy(null);
    }
  };

  const disconnect = async (client: McpClient) => {
    const clientType = client.client_type;
    setBusy(clientType);
    setErrors((prev) => ({ ...prev, [clientType]: null }));
    try {
      await removeRawMcpEntry(clientType);
      setAdded((prev) => {
        const next = { ...prev };
        delete next[clientType];
        return next;
      });
      queryClient.invalidateQueries({ queryKey: ["mcp-clients"] });
    } catch (err) {
      setErrors((prev) => ({ ...prev, [clientType]: { ...describeSetupError(err), mode: "setup" } }));
    } finally {
      setBusy(null);
    }
  };

  const notChecked = (
    <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-warning-text)" }}>
      {t("connectMatrix.notChecked")}
    </span>
  );

  // A measured `no` gets no row at all: those tools are named once, on the
  // "Also works with" line. A failed look still gets its row and its switch —
  // withholding the action would be the same false negative wearing a
  // different coat.
  const trailing = (client: McpClient) => {
    const clientType = client.client_type;
    const isBusy = busy === clientType || connectingAll;
    if (added[clientType]) {
      // A plugin cannot be taken back from here; the row only says what is
      // left to do (restart the tool).
      if (isPluginClient(clientType)) return null;
      return (
        <Toggle
          enabled
          valueUnknown={busy === clientType}
          unknownState="saving"
          onToggle={() => void disconnect(client)}
          aria-label={t("connectMatrix.connectTool", { name: client.name })}
          aria-describedby={clientRowDescId(clientType)}
        />
      );
    }
    // The entry exists and would not start: one action, and it is a fix.
    if (repairReasonOf(client) !== null) {
      return (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => setUp(client, "repair")}
          disabled={isBusy}
          aria-describedby={clientRowDescId(clientType)}
        >
          {busy === clientType ? t("connectMatrix.repairing") : t("connectMatrix.repair")}
        </Button>
      );
    }
    // Already connected; only the skills are missing. A switch that is off
    // would read as "not connected", so this is a plain button.
    if (needsSkillsOnly(client)) {
      return (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => setUp(client)}
          disabled={isBusy}
          aria-describedby={clientRowDescId(clientType)}
        >
          {busy === clientType ? t("connectMatrix.settingUp") : t("connectMatrix.addSkills")}
        </Button>
      );
    }
    // Round 6, D6a. A write that could produce a duplicate registration must
    // not be offered as the SAME one-tap switch a measured `no` gets. It stays
    // a button labelled "Set up anyway" that points at the body line saying
    // what could not be read — so a screen reader hears the risk as part of
    // the control, not several nodes away from it.
    const control =
      unreadPluginWriteRisk(client) !== null ? (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => setUp(client)}
          disabled={isBusy}
          aria-describedby={clientRowDescId(clientType)}
        >
          {busy === clientType ? t("connectMatrix.settingUp") : t("connectMatrix.setUpAnyway")}
        </Button>
      ) : (
        <Toggle
          enabled={false}
          valueUnknown={isBusy}
          unknownState="saving"
          onToggle={() => void setUp(client)}
          aria-label={t("connectMatrix.connectTool", { name: client.name })}
          aria-describedby={clientRowDescId(clientType)}
        />
      );
    if (client.detected.kind === "unreadable") {
      return (
        <div className="flex items-center gap-2">
          {notChecked}
          {control}
        </div>
      );
    }
    return control;
  };

  const notInstalledNames = (clients ?? [])
    .filter((client) => client.detected.kind === "no" && !added[client.client_type])
    .map((client) => client.name);
  const alsoWorksWith =
    notInstalledNames.length > 0 ? (
      <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)", lineHeight: "1.5", margin: 0 }}>
        {t("connectMatrix.alsoWorksWith", { names: notInstalledNames.join(t("connectMatrix.nameSeparator")) })}
      </p>
    ) : null;

  const rows = actionable.filter(
    (client) => client.detected.kind !== "no" || added[client.client_type],
  );

  if (clients && rows.length === 0) {
    if (hideWhenComplete && clients.length > 0) return null;
    // A written config is not a live connection. Clients that are configured
    // but whose family never appeared in the roster are "added", not
    // "connected", so the note claims only what is measured. The restart
    // instruction is not repeated here: it lives on that tool's own row in the
    // list above. An undetected client is never counted: nothing measured says
    // it is there.
    const pendingRestart = clients.filter(
      (client) =>
        readingIsYes(client.already_configured) &&
        !readingIsNo(client.detected) &&
        !(seen?.has(familyOfClient(client.client_type)) ?? false),
    );
    return section(
      <div className="flex flex-col" style={{ gap: "6px" }}>
        {title}
        <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
          {t(pendingRestart.length === 0 ? "connectMatrix.allConnected" : "connectMatrix.allAdded")}
        </span>
        {alsoWorksWith}
      </div>,
    );
  }

  // "Connect all" takes only the rows a single tap would set up with no
  // question left: a measured install, no unread plugin state, not a leftover
  // settings folder, not already done or already connected. Anything else
  // keeps its own control.
  const connectAllTargets = rows.filter(
    (client) =>
      !added[client.client_type] &&
      !needsSkillsOnly(client) &&
      readingIsYes(client.detected) &&
      !isConfigOnly(client) &&
      repairReasonOf(client) === null &&
      unreadPluginWriteRisk(client) === null,
  );
  const connectAll = async () => {
    setConnectingAll(true);
    try {
      // One at a time: two tools can share a skills folder (Antigravity and
      // Gemini CLI, GitHub Copilot and VS Code), and every write takes a
      // backup first.
      for (const client of connectAllTargets) await setUp(client);
    } finally {
      setConnectingAll(false);
    }
  };

  return section(
    <div className="flex flex-col" style={{ gap: "8px" }}>
      {title}
      {connectAllTargets.length >= 2 && (
        <Card>
          <div className="flex flex-col" style={{ gap: "10px" }}>
            <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-md)", fontWeight: 600, color: "var(--mem-text)", margin: 0 }}>
              {t("connectMatrix.foundTools", { count: connectAllTargets.length })}
            </p>
            <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)", margin: 0 }}>
              {connectAllTargets.map((client) => client.name).join(t("connectMatrix.nameSeparator"))}
            </p>
            <div className="flex items-center flex-wrap" style={{ gap: "10px" }}>
              <Button type="button" variant="primary" size="sm" onClick={() => void connectAll()} disabled={connectingAll || busy !== null}>
                {connectingAll ? t("connectMatrix.settingUp") : t("connectMatrix.connectAll")}
              </Button>
              <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
                {t("connectMatrix.connectAllHint")}
              </span>
            </div>
          </div>
        </Card>
      )}
      <Card padding="none">
        <div className="flex flex-col py-1">
          {rows.map((client) => {
            const duplicateRiskError = unreadPluginWriteRisk(client);
            const addedHere = added[client.client_type] === true;
            const failure = errors[client.client_type];
            const skillsNote =
              addedHere || repairReasonOf(client) !== null || !isSkillsClient(client)
                ? null
                : needsSkillsOnly(client)
                  ? t("connectMatrix.needsSkills")
                  : t("connectMatrix.addsSkills");
            return (
              <ClientRow
                key={client.client_type}
                client={client}
                bare
                configured={client.already_configured}
                status={addedHere || needsSkillsOnly(client) ? statusOf(client.client_type) : null}
                error={
                  failure
                    ? `${t(
                        failure.mode === "repair" ? "connectMatrix.repairFailed" : "connectMatrix.addFailed",
                        { name: client.name },
                      )} ${t(setupErrorHeadingKey(failure.kind))}`
                    : null
                }
                errorDetail={failure?.detail}
                warning={
                  // Two independent non-fatal notes, and they can both be live:
                  // the plugin state could not be read BEFORE the write, and the
                  // binary search skipped an input DURING it. Joined, not ranked.
                  [
                    duplicateRiskError
                      ? t("connectMatrix.pluginStateUnknownBeforeWrite", { error: duplicateRiskError })
                      : null,
                    warnings[client.client_type] || null,
                  ]
                    .filter(Boolean)
                    .join(" ") || null
                }
                trailing={trailing(client)}
              >
                {skillsNote && (
                  <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)", lineHeight: "1.5", margin: 0 }}>
                    {skillsNote}
                  </p>
                )}
                {addedHere && (
                  <p
                    role="status"
                    style={{
                      fontFamily: "var(--mem-font-body)",
                      fontSize: "var(--mem-text-sm)",
                      color: "var(--mem-text-tertiary)",
                      lineHeight: "1.5",
                      margin: 0,
                    }}
                  >
                    {repaired[client.client_type]
                      ? t("connectMatrix.repairedRestart", { name: client.name })
                      : statusOf(client.client_type) === "connected"
                        ? t("connectMatrix.connectedHint", { name: client.name })
                        : t("connectMatrix.addedRestart", { name: client.name })}
                  </p>
                )}
              </ClientRow>
            );
          })}
        </div>
      </Card>
      {alsoWorksWith}
    </div>,
  );
}
