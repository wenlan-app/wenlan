// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clipboardWrite, type McpClient } from "../../lib/tauri";
import { familyDisplayName } from "../../lib/agents";
import type { Reading } from "../../lib/reading";
import { StatusChip } from "../memory/settings/primitives";
import { ConnectionChip, type ConnectionStatus } from "./connectionState";
import { entryUnreadable, isConfigOnly, repairReasonOf } from "./clientHealth";
import { familyOfClient } from "./connectionState";

interface ClientRowProps {
  client: McpClient;
  /** Settings shows the mono config path under the name; the wizard hides it
   *  (noise for a first-run user). */
  showConfigPath?: boolean;
  /** Left slot in the title row (wizard checkbox). Wrapped together with the
   *  name in a <label> so clicking the name also toggles the checkbox. */
  leading?: ReactNode;
  /** Right slot in the title row (Settings "Set up" button / "Not installed"
   *  text). Rendered outside the label — never nested inside it, so a click
   *  here can never also toggle a `leading` checkbox. */
  trailing?: ReactNode;
  /** Body under the title row (the wizard's "already set up" note, etc).
   *  Always a sibling of the label — never nested inside it — for the same
   *  reason. */
  children?: ReactNode;
  /** The failure sentence, already localized by the caller. */
  error?: string | null;
  /** The raw failure text copied by the row's Copy details action. */
  errorDetail?: string | null;
  /** Other raw diagnostics that must remain available without appearing in
   *  the row's visible copy. */
  technicalDetails?: readonly (string | null | undefined)[];
  /** Non-fatal, and rendered in the BODY the row's `descId` covers, so a
   *  caller can point its button's `aria-describedby` at it.
   *
   *  This is where a stated unknown goes: "the plugin state could not be read,
   *  so setting up now may register Wenlan twice" (round 6, D6a), and "the
   *  entry was written, but one resolver input could not be determined"
   *  (D3's boundary). Neither is an error — nothing failed — and neither may
   *  be silent, which is the only other thing this row used to be able to do
   *  with them. */
  warning?: string | null;
  /** Three-valued — see [`Reading`]. A measured `yes` renders the "Added"
   *  chip (honest: it came from actually parsing the config file, and a
   *  config says nothing about a live connection), a
   *  failed read renders an "unknown" chip, and a measured `no` renders
   *  nothing. It used to be a boolean, so "we could not read the config" and
   *  "the config has no Wenlan entry" rendered identically — as nothing. */
  configured: Reading;
  /** What the roster knows about this tool, when the caller has asked it:
   *  `connected` once the tool has called Wenlan, `added` while only Wenlan's
   *  side is done. It replaces the config-derived chip, because the config
   *  alone can never say `connected`. */
  status?: ConnectionStatus | null;
  /** No card of its own: the row sits in a list that already has one
   *  (Settings' Add a tool). */
  bare?: boolean;
  /** Highlights the card border — wizard rows use this when their checkbox
   *  is checked. */
  selected?: boolean;
}

/** Deterministic id for a row's body (config path, error, status note) so a
 *  caller-built `leading` checkbox can wire `aria-describedby` to it without a
 *  prop round-trip through this component. */
export function clientRowDescId(clientType: string): string {
  return `client-row-desc-${clientType}`;
}

/** One row renderer shared by the wizard's ConnectStep and Settings'
 *  ClientSetupList — sharing the row (not the whole list) is what the
 *  redesign spec (docs/superpowers/plans/2026-07-12-connect-step-redesign.md
 *  §4) uses to stop copy/behavior drifting between the two surfaces. */
export default function ClientRow({
  client,
  showConfigPath,
  leading,
  trailing,
  children,
  error,
  errorDetail,
  technicalDetails = [],
  warning,
  configured,
  status,
  selected,
  bare = false,
}: ClientRowProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  // Deterministic (not useId-generated) so the caller's `leading` checkbox —
  // built independently, before this component ever renders it — can wire
  // aria-describedby to the same id without a prop round-trip.
  const descId = clientRowDescId(client.client_type);
  // A detection that FAILED earns a line of its own. The two things this
  // replaces both turned it into a confident negative: the wizard DROPPED
  // any row whose `detected` was falsy, and the Settings list labelled it
  // "Not installed".
  const detectionFailed = client.detected.kind === "unreadable" ? client.detected.error : null;
  // What the daemon's own checks add. Each is chosen by `kind` / `reason`, never
  // by the Rust `detail`, which is English-only text.
  const repairReason = repairReasonOf(client);
  const entryCannotBeRead = entryUnreadable(client);
  const configOnly = isConfigOnly(client);
  const displayName = familyDisplayName(familyOfClient(client.client_type));
  const subtitleKey =
    client.client_type === "codex_cli"
      ? "connectMatrix.subtitleCodex"
      : client.client_type === "kimi_desktop"
        ? "connectMatrix.subtitleKimiDesktop"
        : client.client_type === "kimi_code"
          ? "connectMatrix.subtitleKimiCode"
          : null;
  const rawDetails = [
    detectionFailed,
    client.already_configured.kind === "unreadable" ? client.already_configured.error : null,
    client.entry_health?.kind === "unreadable" ? client.entry_health.error : null,
    errorDetail,
    ...technicalDetails,
  ].filter((detail, index, all): detail is string => Boolean(detail) && all.indexOf(detail) === index);
  const hasBody = Boolean(
    showConfigPath ||
      subtitleKey ||
      children ||
      error ||
      warning ||
      detectionFailed ||
      entryCannotBeRead ||
      configOnly ||
      rawDetails.length > 0,
  );

  const nameBadges = (
    <div className="flex items-center gap-2 flex-wrap min-w-0">
      <span
        className="truncate"
        style={{
          fontFamily: "var(--mem-font-body)",
          fontSize: "var(--mem-text-md)",
          fontWeight: 500,
          color: "var(--mem-text)",
        }}
      >
        {displayName}
      </span>
      {repairReason ? (
        // The entry is there but would not start, so "Added" (restart to
        // finish) would send the user to do the wrong thing.
        <StatusChip state={{ kind: "down" }} label={t("connectMatrix.needsRepair")} />
      ) : status ? (
        <ConnectionChip status={status} />
      ) : (
        configured.kind === "yes" && (
          // A written entry, read back from the config file. That is "added",
          // not "connected": nothing here has seen the tool call Wenlan.
          <ConnectionChip status="added" />
        )
      )}
      {configured.kind === "unreadable" && (
        // A read that FAILED. Not the "up" chip (we did not see a Wenlan
        // entry) and not silence (silence is what a measured `no` renders,
        // and reading them the same is the whole defect).
        //
        // When the row already carries the localized "can't read this tool's
        // settings" line, the raw OS error in the chip says the same thing
        // again in English, so the chip keeps only its localized label. Any
        // other unreadable entry keeps the detail: nothing else explains it.
        <StatusChip
          state={{
            kind: "unknown",
            detail: undefined,
          }}
          label={t("connectMatrix.configuredUnknown")}
        />
      )}
    </div>
  );

  const bodyBlock = hasBody ? (
    <div
      id={descId}
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "6px",
        paddingLeft: leading ? "28px" : 0,
      }}
    >
      {showConfigPath && client.config_path && (
        <p
          className="truncate"
          style={{
            fontFamily: "var(--mem-font-mono)",
            fontSize: "var(--mem-text-2xs)",
            color: "var(--mem-text-tertiary)",
            margin: 0,
          }}
        >
          {client.config_path}
        </p>
      )}
      {subtitleKey && (
        <p
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-text-tertiary)",
            margin: 0,
          }}
        >
          {t(subtitleKey)}
        </p>
      )}
      {detectionFailed && (
        <p
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-status-warning-text)",
            margin: 0,
          }}
        >
          {t("connectMatrix.detectionUnknown")}
        </p>
      )}
      {configOnly && (
        // Not a claim that the app is installed: only its settings were found.
        <p
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-text-tertiary)",
            margin: 0,
          }}
        >
          {t("connectMatrix.configOnly")}
        </p>
      )}
      {entryCannotBeRead && (
        <p
          data-testid={`client-row-entry-unreadable-${client.client_type}`}
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-status-warning-text)",
            margin: 0,
          }}
        >
          {t("connectMatrix.entryUnreadable")}
        </p>
      )}
      {warning && (
        <p
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-status-warning-text)",
            margin: 0,
          }}
        >
          {warning}
        </p>
      )}
      {children}
      {error && (
        <div
          role="alert"
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-status-danger-text)",
            margin: 0,
          }}
        >
          <p style={{ margin: 0 }}>{error}</p>
        </div>
      )}
      {rawDetails.length > 0 && (
        <button
          type="button"
          className="w-fit text-[length:var(--mem-text-xs)] text-[var(--mem-text-secondary)] hover:text-[var(--mem-text)] focus-visible:outline-2 focus-visible:outline-[var(--mem-focus-ring)] focus-visible:outline-offset-2 rounded-[var(--mem-radius-sm)]"
          onClick={() => {
            void clipboardWrite(rawDetails.join("\n\n"))
              .then(() => setCopied(true))
              .catch(() => setCopied(false));
          }}
        >
          {copied ? t("connectMatrix.copied") : t("connectMatrix.copyDetails")}
        </button>
      )}
    </div>
  ) : null;

  // In a list that already has its card, the control sits centred against the
  // whole row (name and the lines under it), the way a settings list reads.
  if (bare) {
    return (
      <div
        data-connection-row
        className="flex items-center justify-between gap-3 rounded-[var(--mem-radius-md)] px-3 py-2.5"
      >
        <div className="flex flex-col gap-1 min-w-0">
          {nameBadges}
          {bodyBlock}
        </div>
        {trailing && <div className="flex h-7 shrink-0 items-center">{trailing}</div>}
      </div>
    );
  }

  return (
    <div
      className="flex flex-col gap-2 rounded-xl px-4 py-3"
      style={{
        backgroundColor: "var(--mem-surface)",
        border: `1px solid ${selected ? "var(--mem-accent-indigo-border)" : "var(--mem-border)"}`,
      }}
    >
      <div className="flex items-start justify-between gap-3">
        {leading ? (
          <label className="flex items-start gap-3 min-w-0" style={{ cursor: "pointer" }}>
            {leading}
            {nameBadges}
          </label>
        ) : (
          <div className="flex items-start gap-3 min-w-0">{nameBadges}</div>
        )}
        {trailing && <div className="shrink-0">{trailing}</div>}
      </div>

      {bodyBlock}
    </div>
  );
}
