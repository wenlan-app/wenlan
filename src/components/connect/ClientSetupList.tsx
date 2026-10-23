// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import type { ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { detectMcpClients, type McpClient } from "../../lib/tauri";
import { readingIsNo, readingIsYes } from "../../lib/reading";
import { describeSetupError, setupErrorHeadingKey, type DescribedSetupError } from "../../lib/setupErrors";
import { connectClient } from "./connectClient";
import { unreadPluginWriteRisk } from "./setupRisk";
import ClientRow, { clientRowDescId } from "./ClientRow";
import { familyOfClient, type ConnectionStatus } from "./connectionState";
import { entryUnreadable, repairReasonOf } from "./clientHealth";
import { Button, SectionHeader } from "../memory/settings/primitives";

/** Apps & CLIs group. Every detected client has the same one-click "Set up" —
 *  what that button *does* differs by client, and `isPluginClient` is the only
 *  thing that decides: Claude Code and Codex get the Wenlan plugin (which
 *  registers the MCP server itself), everyone else gets an MCP config write.
 *  Writing a config for a plugin client would register Wenlan twice, so this
 *  surface obeys the same invariant the wizard does.
 *
 *  `connectedFamilies` (the tool families the roster above already shows)
 *  is the single source of truth for what to hide here: a client whose family
 *  already has an identity is represented above, so re-listing it — even when
 *  its own config file looks unconfigured — is the duplication the user
 *  vetoed.
 *
 *  `seenFamilies` is the narrower set whose identities have actually called
 *  Wenlan. It decides Added versus Connected. A tool the user adds from this
 *  list stays in place, saying "Added. Restart it to finish", and flips to
 *  Connected when its family first shows up in `seenFamilies`, instead of
 *  vanishing the moment its config is written. */
export default function ClientSetupList({
  connectedFamilies,
  seenFamilies,
  hideWhenComplete = false,
}: {
  connectedFamilies?: Set<string>;
  seenFamilies?: Set<string>;
  /** Hide a completed Add a tool section in compact Settings surfaces. */
  hideWhenComplete?: boolean;
} = {}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [errors, setErrors] = useState<
    Record<string, (DescribedSetupError & { mode: "setup" | "repair" }) | null>
  >({});
  // Clients this list added since it mounted. They keep their row.
  const [added, setAdded] = useState<Record<string, true>>({});
  // The subset of those that were repaired rather than set up, which changes
  // what the row says next ("Repaired. Restart X to finish.").
  const [repaired, setRepaired] = useState<Record<string, true>>({});
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

  const notInstalled = (
    <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
      {t("connectMatrix.notDetected")}
    </span>
  );

  const notChecked = (
    <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-warning-text)" }}>
      {t("connectMatrix.notChecked")}
    </span>
  );

  // Three branches, because there are three readings. Only a MEASURED `no`
  // says "Not installed"; a failed look says so, and still offers the button —
  // withholding the action would be the same false negative wearing a
  // different coat.
  const trailing = (client: McpClient) => {
    // A tool added from this list has nothing left to press.
    if (added[client.client_type]) return null;
    if (client.detected.kind === "no") return notInstalled;
    // The entry exists and would not start: one action, and it is a fix.
    if (repairReasonOf(client) !== null) {
      return (
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => setUp(client, "repair")}
          disabled={busy === client.client_type}
          aria-describedby={clientRowDescId(client.client_type)}
        >
          {busy === client.client_type ? t("connectMatrix.repairing") : t("connectMatrix.repair")}
        </Button>
      );
    }
    // Round 6, D6a. A write that could produce a duplicate registration must
    // not be offered as the SAME unqualified action a measured `no` gets. The
    // label changes to "Set up anyway", and the button points at the body line
    // that says what could not be read — so a screen reader hears the risk as
    // part of the button, not several nodes away from it.
    const duplicateRisk = unreadPluginWriteRisk(client) !== null;
    const button = (
      <Button
        type="button"
        variant="secondary"
        size="sm"
        onClick={() => setUp(client)}
        disabled={busy === client.client_type}
        aria-describedby={duplicateRisk ? clientRowDescId(client.client_type) : undefined}
      >
        {busy === client.client_type
          ? t("connectMatrix.settingUp")
          : duplicateRisk
            ? t("connectMatrix.setUpAnyway")
            : t("connectMatrix.setUp")}
      </Button>
    );
    if (client.detected.kind === "unreadable") {
      return (
        <div className="flex items-center gap-2">
          {notChecked}
          {button}
        </div>
      );
    }
    return button;
  };

  if (clients && actionable.length === 0) {
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
    if (pendingRestart.length === 0) {
      return (
        section(
          <>
            {title}
            <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
              {t("connectMatrix.allConnected")}
            </span>
          </>,
        )
      );
    }
    return (
      section(
        <>
          {title}
          <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
            {t("connectMatrix.allAdded")}
          </span>
        </>,
      )
    );
  }

  return section(
    <div className="flex flex-col" style={{ gap: "8px" }}>
      {title}
      {actionable.map((client) => {
        const duplicateRiskError = unreadPluginWriteRisk(client);
        const addedHere = added[client.client_type] === true;
        const failure = errors[client.client_type];
        return (
          <ClientRow
            key={client.client_type}
            client={client}
            configured={client.already_configured}
            status={addedHere ? statusOf(client.client_type) : null}
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
    </div>,
  );
}
