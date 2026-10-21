// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  getWireState,
  removeLegacyMcpEntry,
  removeRawMcpEntry,
  startDaemonSidecar,
  type ActivityResponse,
  type ActivityStep,
  type BinaryCandidate,
  type BinaryWire,
  type ClientWire,
  type DaemonWire,
  type JobBinding,
  type SidecarStopOutcome,
  type WireState,
} from "../../../../lib/tauri";
import { readingIsNo, readingIsYes, type Reading } from "../../../../lib/reading";
import { relativeTime } from "../../../../lib/relativeTime";
import { useActivity } from "../../../../lib/useActivity";
import {
  knownLane,
  knownState,
  onDemandPagesWaiting,
  laneKey,
  ROUTE_JOBS,
  STEP_UNIT,
  type KnownActivityAssetKind,
  type KnownActivityStep,
} from "../../../../lib/activitySentence";
import "./diagnosticsContent.css";
import { Button, Card, SectionHeader, Skeleton, StatusChip, type ProbeState } from "../primitives";

/** A daemon from before `/api/activity` answers it with a plain 404. */
function isOldDaemonError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("/api/activity") && message.includes("404");
}

// ── Wiring card ───────────────────────────────────────────────────────────
// What the background work card below can't tell you: whether the plumbing
// works at all. `getWireState()` never rejects on a down daemon
// (`daemon.reachable: false` instead), so isError here only means the IPC
// call itself failed.

/** `route` arrives from Rust as a bare tag (`plugin` | `config` | `skip`).
 *  It is never rendered raw: `plugin` is a word we don't say about Wenlan, and
 *  the i18n banned-word guard can't see a value that isn't in resources.ts. An
 *  unrecognised tag falls through to itself so a new route shows up rather than
 *  silently vanishing. */
const ROUTE_LABEL_KEYS = {
  plugin: "settings.diagnostics.wiring.routePlugin",
  config: "settings.diagnostics.wiring.routeConfig",
  skip: "settings.diagnostics.wiring.routeSkip",
  unknown: "settings.diagnostics.wiring.routeUnknown",
} as const;

function routeLabel(t: TFunction, route: string): string {
  const key = ROUTE_LABEL_KEYS[route as keyof typeof ROUTE_LABEL_KEYS];
  return key ? t(key) : route;
}

/** How one client detection reads on screen. THREE labels, because there are
 *  three readings: a look that failed is not a client that is absent, and
 *  "Not detected" said both. */
function detectionLabel(t: TFunction, detected: Reading): string {
  switch (detected.kind) {
    case "yes":
      return t("settings.diagnostics.wiring.clientDetected");
    case "no":
      return t("settings.diagnostics.wiring.clientNotDetected");
    case "unreadable":
      return t("settings.diagnostics.wiring.clientUnreadable");
  }
}

/** How one candidate reads on screen. "Missing" used to cover every non-`file`
 *  state, including the one where the OS refused to answer — the boolean
 *  collapse, rendered. A path that could not be looked at is not a missing
 *  path, and the user chasing a broken setup needs to know which it is. */
function candidateLabel(t: TFunction, state: BinaryCandidate["state"]): string {
  switch (state.kind) {
    case "file":
      return t("settings.diagnostics.wiring.candidateFound");
    case "unreadable":
      return t("settings.diagnostics.wiring.candidateUnreadable");
    case "not_a_file":
    case "not_executable":
      return t("settings.diagnostics.wiring.candidateUnusable");
    case "absent":
      return t("settings.diagnostics.wiring.candidateMissing");
    default:
      return t("settings.diagnostics.unknown");
  }
}

function candidateState(candidate: BinaryCandidate, anyFound: boolean): ProbeState {
  switch (candidate.state.kind) {
    case "file":
      return { kind: "up" };
    case "unreadable":
      return { kind: "unknown" };
    case "absent":
    case "not_a_file":
    case "not_executable":
      return anyFound ? { kind: "idle" } : { kind: "down" };
    default:
      return { kind: "unknown" };
  }
}

/** C1.7. `sidecar_spawned_on_unknown_owner`, the job binding and the last
 *  stop outcome all reached `DaemonWire` and stopped there. `daemon_start`
 *  maps both `Spawn` and `SpawnOnUnknownOwner` to the same `Started` result,
 *  so the difference between "launchd was measured not to own the daemon" and
 *  "nobody could tell" was recorded and never rendered — which made the claim
 *  that the third value reaches a user-visible outcome false. These render it.
 *
 *  All three are about a daemon that outlives, or duplicates, the one the user
 *  thinks they have; none of them is visible anywhere else in the app. */
function sidecarBindingLabel(t: TFunction, binding: JobBinding): string {
  switch (binding.state) {
    case "bound":
      return t("settings.diagnostics.wiring.sidecarBound");
    case "unbound":
      return t("settings.diagnostics.wiring.sidecarUnbound");
    default:
      return t("settings.diagnostics.wiring.sidecarNotSupported");
  }
}

/** `null` for `no_sidecar`: nothing was stopped and nothing needed to be, so
 *  there is no outcome worth a row. Every other outcome gets one — including
 *  `could_not_measure`, which is the whole reason the field exists. */
function lastStopLabel(t: TFunction, stop: SidecarStopOutcome): string | null {
  switch (stop.outcome) {
    case "ended":
      return t("settings.diagnostics.wiring.lastStopEnded");
    case "still_running":
      return t("settings.diagnostics.wiring.lastStopStillRunning");
    case "could_not_measure":
      return t("settings.diagnostics.wiring.lastStopUnverified");
    default:
      return null;
  }
}

// ── Technical details sections ──────────────────────────────────────────

function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-diagnostics-detail-section">
      <SectionHeader label={title} />
      <Card padding="rows">{children}</Card>
    </section>
  );
}

function WireLoading({ announce = false }: { announce?: boolean }) {
  const { t } = useTranslation();
  return (
    <div
      className="settings-diagnostics-detail-loading"
      role={announce ? "status" : "group"}
      aria-busy="true"
      aria-label={t("settings.diagnostics.wiring.loading")}
    >
      {announce && <span className="sr-only">{t("settings.diagnostics.wiring.loading")}</span>}
      <Skeleton width="40%" height={14} />
      <Skeleton width="65%" height={10} />
    </div>
  );
}

function ActivityPending({ loading }: { loading: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="settings-diagnostics-detail-empty">
      <StatusChip state={loading ? { kind: "probing" } : { kind: "unknown" }} label={t(loading ? "settings.diagnostics.loading" : "settings.diagnostics.unknown")} />
    </div>
  );
}

function WireUnavailable() {
  const { t } = useTranslation();
  return <p className="settings-diagnostics-detail-empty">{t("settings.diagnostics.wiring.unavailable")}</p>;
}

function StaleSnapshot({ error }: { error: unknown }) {
  const { t } = useTranslation();
  const message = error instanceof Error ? error.message : String(error);
  return (
    <p className="settings-diagnostics-detail-warning">
      {t("settings.diagnostics.overview.lastSnapshot", { error: message })}
    </p>
  );
}

function DaemonStatus({ daemon, onRetry }: { daemon: DaemonWire; onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="settings-diagnostics-detail-grid">
      <div className="settings-diagnostics-detail-row">
        <span className="settings-diagnostics-detail-label">{t("settings.diagnostics.detail.status")}</span>
        <div className="settings-diagnostics-detail-value">
          <StatusChip
            state={daemon.reachable ? { kind: "up" } : { kind: "down" }}
            label={
              daemon.reachable
                ? t("settings.diagnostics.wiring.daemonReachable")
                : t("settings.diagnostics.wiring.daemonUnreachable")
            }
          />
          {!daemon.reachable && daemon.error && (
            <p className="settings-diagnostics-detail-error">{daemon.error}</p>
          )}
          <span className="settings-diagnostics-technical">{daemon.base_url}</span>
          {daemon.version && <span className="settings-diagnostics-detail-muted">{t("settings.diagnostics.wiring.daemonVersion", { version: daemon.version })}</span>}
        </div>
      </div>
      <DaemonSidecarStatus daemon={daemon} />
      {!daemon.reachable && (
        <div className="settings-diagnostics-detail-row">
          <span className="settings-diagnostics-detail-label">{t("settings.diagnostics.wiring.unavailable")}</span>
          <div className="settings-diagnostics-detail-value">
          <Button variant="secondary" size="sm" onClick={onRetry}>{t("settings.diagnostics.wiring.retry")}</Button>
          </div>
        </div>
      )}
    </div>
  );
}

/** See `sidecarBindingLabel`. Renders nothing at all when this app owns no
 *  sidecar and never stopped one — the ordinary case, where there is nothing
 *  to say and a row saying so would be noise. */
function DaemonSidecarStatus({ daemon }: { daemon: DaemonWire }) {
  const { t } = useTranslation();
  const binding = daemon.sidecar_job_binding ?? null;
  const lastStop = daemon.last_sidecar_stop ?? null;
  const lastStopText = lastStop ? lastStopLabel(t, lastStop) : null;
  if (!binding && !lastStopText && !daemon.sidecar_spawned_on_unknown_owner) return null;
  return (
    <div className="settings-diagnostics-detail-row">
      <span className="settings-diagnostics-detail-label">{t("settings.diagnostics.wiring.daemonSidecarTitle")}</span>
      <div className="settings-diagnostics-detail-value">
      {binding && (
        <div className="settings-diagnostics-inline-value">
          <StatusChip
            state={
              binding.state === "bound"
                ? { kind: "up" }
                : binding.state === "unbound"
                  ? { kind: "down" }
                  : { kind: "idle" }
            }
            label={sidecarBindingLabel(t, binding)}
          />
          {binding.state === "unbound" && (
            <span className="settings-diagnostics-technical">
              {binding.reason}
            </span>
          )}
        </div>
      )}
      {lastStop && lastStopText && (
        <div className="settings-diagnostics-inline-value">
          <StatusChip
            // Round 5, D5 residual. `could_not_measure` used to fall into the
            // same `down` as `still_running`, so the colour asserted a
            // negative stop nobody observed — the "unverified" wording was the
            // only thing keeping them apart, and colour is read first. Three
            // outcomes, three tones: ended is measured good, still_running is
            // measured bad, could_not_measure is not a measurement at all.
            state={
              lastStop.outcome === "ended"
                ? { kind: "up" }
                : lastStop.outcome === "could_not_measure"
                  ? { kind: "unknown" }
                  : { kind: "down" }
            }
            label={lastStopText}
          />
          {"reason" in lastStop && (
            <span className="settings-diagnostics-technical">
              {lastStop.reason}
            </span>
          )}
        </div>
      )}
      {daemon.sidecar_spawned_on_unknown_owner && (
        <p className="settings-diagnostics-detail-error">
          {t("settings.diagnostics.wiring.sidecarUnknownOwner")}
        </p>
      )}
      </div>
    </div>
  );
}

function McpBinaryStatus({ mcpBinary, onSetupAgent }: {
  mcpBinary: BinaryWire;
  onSetupAgent?: () => void;
}) {
  const { t } = useTranslation();
  // The candidates list is a probe order — most paths are SUPPOSED to be
  // missing as long as one is found. Only flag red (down) when the binary
  // exists nowhere; otherwise a missing candidate is idle, not an alarm.
  const anyFound = mcpBinary.candidates.some((candidate) => candidate.state.kind === "file");
  // A candidate the OS would not answer about is NOT evidence the binary is
  // absent, so it must not trigger missing-binary setup advice: the install
  // may be perfectly fine behind a permission problem. Only a fully measured
  // search offers the button.
  // ...and neither is an input that could not be determined: its candidate
  // paths were never built, so the search did not cover them and their absence
  // from the list below is not evidence of anything.
  // Read off `mcpBinary` itself, not off `unresolved`: an input that could not
  // be determined is a property of the search, and a search that FOUND
  // something can still have one (round 5, D4). Rendering it only when no
  // command was chosen is how it stayed invisible in the case where the
  // command beside it looks like a complete answer.
  const undetermined = mcpBinary.undetermined ?? [];
  const candidateKinds = ["file", "not_a_file", "not_executable", "absent", "unreadable"];
  const hasUnknownCandidate = mcpBinary.candidates.some(
    (candidate) => !candidateKinds.includes(candidate.state.kind),
  );
  const searchIncomplete =
    mcpBinary.candidates.some((candidate) => candidate.state.kind === "unreadable") ||
    undetermined.length > 0 ||
    hasUnknownCandidate;
  return (
    <div className="settings-diagnostics-detail-grid">
      {mcpBinary.candidates.flatMap((candidate) =>
        candidate.state.kind === "unreadable"
          ? [
              <p
                key={`candidate-error:${candidate.source}:${candidate.path}`}
                className="settings-diagnostics-detail-warning"
              >
                {candidateLabel(t, candidate.state)}: {candidate.state.error}
              </p>,
            ]
          : [],
      )}
      {undetermined.map((input) => (
        <p key={input.input} className="settings-diagnostics-detail-warning">
          {t("settings.diagnostics.wiring.candidateNotChecked")}: {t("settings.diagnostics.wiring.binaryUndetermined", { ...input })}
        </p>
      ))}
      <div className="settings-diagnostics-detail-row">
        <span className="settings-diagnostics-detail-label">{t("settings.diagnostics.detail.selectedExecutable")}</span>
        {mcpBinary.command === null ? (
          <p className="settings-diagnostics-detail-error">
            {mcpBinary.unresolved?.message ?? t("settings.diagnostics.wiring.binaryUnresolved")}
          </p>
        ) : (
          <span data-testid="diagnostics-selected-executable" className="settings-diagnostics-technical">
            {[mcpBinary.command, ...mcpBinary.args].join(" ")}
          </span>
        )}
      </div>
      {mcpBinary.candidates.length > 0 && (
        <div className="settings-diagnostics-candidate-list">
          <div className="settings-diagnostics-candidate-grid settings-diagnostics-candidate-heading">
            <span>{t("settings.diagnostics.detail.candidateSource")}</span>
            <span>{t("settings.diagnostics.detail.candidatePath")}</span>
            <span>{t("settings.diagnostics.detail.candidateStatus")}</span>
          </div>
          {/* Keep source as part of the identity: two slots may point to the same path. */}
          {mcpBinary.candidates.map((candidate) => (
            <div key={`${candidate.source}:${candidate.path}`} className="settings-diagnostics-candidate-grid">
              <span className="settings-diagnostics-technical">{candidate.source}</span>
              <span className="settings-diagnostics-technical settings-diagnostics-path">{candidate.path}</span>
              <StatusChip state={candidateState(candidate, anyFound)} label={candidateLabel(t, candidate.state)} />
            </div>
          ))}
        </div>
      )}
      {/* Tool setup is an optional in-app route. Opening it leaves the
          first-launch completion flag intact and makes no reinstall claim. */}
      {!anyFound && !searchIncomplete && onSetupAgent && (
        <div className="settings-diagnostics-detail-row">
          <span />
          <Button
            variant="secondary"
            size="sm"
            onClick={onSetupAgent}
          >
            {t("settings.diagnostics.wiring.reinstallViaSetup")}
          </Button>
        </div>
      )}
    </div>
  );
}

/** The double-registration warnbox, now with its one-action fix: remove the
 *  raw MCP entry (`removeRawMcpEntry`). Its own component so the mutation's
 *  hooks stay at a component top level rather than inside the clients `.map`.
 *  On success the wire query is invalidated so the box re-renders against
 *  fresh state (and disappears once the duplicate is gone). A failure is
 *  surfaced verbatim — the same policy the daemon error uses. */
function DoubleRegistrationWarning({ client }: { client: ClientWire }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const removeMutation = useMutation({
    mutationFn: () => removeRawMcpEntry(client.client_type),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["wireState"] }),
  });
  const errorMessage =
    removeMutation.error instanceof Error
      ? removeMutation.error.message
      : removeMutation.error != null
        ? String(removeMutation.error)
        : null;
  return (
    <div
      className="settings-diagnostics-client-warning flex items-start gap-2 mt-1"
      style={{
        background: "var(--mem-status-danger-bg)",
        border: "1px solid var(--mem-status-danger-border)",
        borderRadius: "var(--mem-radius-md)",
        padding: "8px 10px",
      }}
    >
      <svg aria-hidden="true" className="w-3.5 h-3.5 text-[var(--mem-status-danger-text)] shrink-0 mt-px" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.072 16.5c-.77.833.192 2.5 1.732 2.5z" />
      </svg>
      <div className="flex flex-col gap-2 min-w-0">
        <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)", lineHeight: "1.5" }}>
          {t("settings.diagnostics.wiring.doubleRegistrationBody", { name: client.name })}
        </p>
        <div>
          <Button
            variant="danger"
            size="sm"
            loading={removeMutation.isPending}
            onClick={() => removeMutation.mutate()}
          >
            {t("settings.diagnostics.wiring.removeDuplicate")}
          </Button>
        </div>
        {errorMessage && (
          <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)", lineHeight: "1.5", overflowWrap: "anywhere" }}>
            {errorMessage}
          </p>
        )}
      </div>
    </div>
  );
}

/** The raw+raw duplicate warnbox with its one-action fix: remove only the
 *  legacy `origin` entry, keeping the live `wenlan` one (`removeLegacyMcpEntry`).
 *  Its own component for the same reason as DoubleRegistrationWarning — the
 *  mutation's hooks stay at a component top level, not inside the clients
 *  `.map`. Distinct from that box: this fires only for a no-plugin client
 *  (Cursor, Gemini CLI), where both raw entries are the whole connection, so
 *  the fix must keep `wenlan` rather than drop both. */
function RawDuplicateWarning({ client }: { client: ClientWire }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const removeMutation = useMutation({
    mutationFn: () => removeLegacyMcpEntry(client.client_type),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["wireState"] }),
  });
  const errorMessage =
    removeMutation.error instanceof Error
      ? removeMutation.error.message
      : removeMutation.error != null
        ? String(removeMutation.error)
        : null;
  return (
    <div
      className="settings-diagnostics-client-warning flex items-start gap-2 mt-1"
      style={{
        background: "var(--mem-status-danger-bg)",
        border: "1px solid var(--mem-status-danger-border)",
        borderRadius: "var(--mem-radius-md)",
        padding: "8px 10px",
      }}
    >
      <svg aria-hidden="true" className="w-3.5 h-3.5 text-[var(--mem-status-danger-text)] shrink-0 mt-px" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.072 16.5c-.77.833.192 2.5 1.732 2.5z" />
      </svg>
      <div className="flex flex-col gap-2 min-w-0">
        <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)", lineHeight: "1.5" }}>
          {t("settings.diagnostics.wiring.rawDuplicateBody", { name: client.name })}
        </p>
        <div>
          <Button
            variant="danger"
            size="sm"
            loading={removeMutation.isPending}
            onClick={() => removeMutation.mutate()}
          >
            {t("settings.diagnostics.wiring.removeLegacyEntry")}
          </Button>
        </div>
        {errorMessage && (
          <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)", lineHeight: "1.5", overflowWrap: "anywhere" }}>
            {errorMessage}
          </p>
        )}
      </div>
    </div>
  );
}

function ClientsWiring({ clients }: { clients: ClientWire[] }) {
  const { t } = useTranslation();
  return (
    <div className="settings-diagnostics-detail-grid">
      {clients.length === 0 ? (
        <p className="settings-diagnostics-detail-empty">
          {t("settings.diagnostics.wiring.clientsEmpty")}
        </p>
      ) : (
        <div className="settings-diagnostics-client-list">
          {clients.map((client) => {
            // THE valuable finding this card exists to surface: Wenlan
            // registered twice for one client (plugin + a raw MCP entry).
            // Now carries its own one-action fix — see DoubleRegistrationWarning.
            // Both halves must be a MEASURED yes. A warning box that offers a
            // destructive one-click fix ("remove the duplicate entry") may
            // never be raised off a read that failed.
            const doubleRegistered =
              readingIsYes(client.has_plugin) && readingIsYes(client.has_raw_entry);
            // The raw+raw sibling: both `wenlan` and legacy `origin` raw
            // entries in a no-plugin client's config. Gated on a measured
            // `no` for the plugin — not `!has_plugin`, which fired this box on
            // an unread plugin state — so it never double-fires with
            // doubleRegistered (a plugin client is handled by the box above,
            // whose fix removes both raw entries).
            const rawDuplicate =
              readingIsNo(client.has_plugin) && readingIsYes(client.has_raw_duplicate);
            // A detection that could not be made gets the neutral chip, not
            // the "idle" one that means "we looked and it is not here".
            const detectedState: Parameters<typeof StatusChip>[0]["state"] =
              client.detected.kind === "yes"
                ? { kind: "up" }
                : client.detected.kind === "no"
                  ? { kind: "idle" }
                  : { kind: "unknown" };
            const unreadableReads = [
              ["detected", client.detected],
              ["has_plugin", client.has_plugin],
              ["has_raw_entry", client.has_raw_entry],
              ["has_raw_duplicate", client.has_raw_duplicate],
            ] as const;
            return (
              <div key={client.client_type} className="settings-diagnostics-client">
                <span className="settings-diagnostics-client-name">{client.name}</span>
                <div className="settings-diagnostics-client-detection">
                  <StatusChip state={detectedState} label={detectionLabel(t, client.detected)} />
                  <span className="settings-diagnostics-client-route">{routeLabel(t, client.route)}</span>
                </div>
                {unreadableReads.map(([field, reading]) => {
                  if (reading.kind !== "unreadable") return null;
                  return (
                    <p key={field} className="settings-diagnostics-detail-error">
                      <span className="settings-diagnostics-technical">{field}</span>: {t("settings.diagnostics.wiring.clientUnreadableDetail", { error: reading.error })}
                    </p>
                  );
                })}
                {doubleRegistered && <DoubleRegistrationWarning client={client} />}
                {rawDuplicate && <RawDuplicateWarning client={client} />}
                <span className="settings-diagnostics-detail-label settings-diagnostics-client-path-label">
                  {t("settings.diagnostics.detail.configurationPath")}
                </span>
                <span className="settings-diagnostics-technical settings-diagnostics-path settings-diagnostics-client-path">
                  {client.config_path ?? t("settings.diagnostics.wiring.clientPathUnknown")}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function WiringError({ onRetry }: { onRetry: () => void }) {
  const { t } = useTranslation();
  const [failure, setFailure] = useState<string | null>(null);
  // Diagnostics is reached when the daemon is down, but Retry only re-probes —
  // it can't bring a dead daemon back. Start actually respawns the sidecar
  // (guarded app-side against double-spawn), then re-probes the wiring.
  const start = useMutation({
    mutationFn: startDaemonSidecar,
    onSuccess: (result) => {
      if (result.status === "failed") {
        setFailure(result.message);
        return;
      }
      // started / already_running / launchd_managed — re-probe the wiring.
      setFailure(null);
      onRetry();
    },
    onError: (err) => setFailure(err instanceof Error ? err.message : String(err)),
  });
  return (
    <div className="px-5 py-4">
      <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-status-danger-text)", lineHeight: "1.5" }}>
        {t("settings.diagnostics.wiring.unavailable")}
      </p>
      <div className="mt-3 flex items-center gap-2">
        <Button variant="secondary" size="sm" onClick={() => start.mutate()} disabled={start.isPending}>
          {start.isPending
            ? t("settings.diagnostics.wiring.starting")
            : t("settings.diagnostics.wiring.start")}
        </Button>
        <Button variant="secondary" size="sm" onClick={onRetry}>
          {t("settings.diagnostics.wiring.retry")}
        </Button>
      </div>
      {failure && (
        <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-status-danger-text)", lineHeight: "1.5", marginTop: "8px" }}>
          {t("settings.diagnostics.wiring.startFailed", { message: failure })}
        </p>
      )}
      {!failure && start.data?.status === "started" && (
        // The daemon needs a moment to bind the port after spawn; the immediate
        // re-probe can still see it down. Tell the user, don't leave them guessing.
        <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)", lineHeight: "1.5", marginTop: "8px" }}>
          {t("settings.diagnostics.wiring.startedHint")}
        </p>
      )}
    </div>
  );
}

// `/api/activity` is shared with Activity. Settings summarizes its health and
// leaves per-step progress to the Activity surface.
function isKnownKind(kind: string): kind is KnownActivityAssetKind {
  return ["memories", "entities", "pages"].includes(kind);
}

/** Known by `STEP_UNIT`, the one table that says what each step counts. */
function isKnownStep(step: ActivityStep): step is KnownActivityStep {
  return Object.prototype.hasOwnProperty.call(STEP_UNIT, step.name);
}

/** Red only for a model someone chose that cannot run. With no model chosen,
 *  Everyday work still runs on the built-in path, so that row stays muted. */
function routeChipState(route: ActivityResponse["everyday"]): ProbeState {
  if (route.available) return { kind: "up" };
  return route.mode === "pinned_unavailable" ? { kind: "down" } : { kind: "idle" };
}

function RoutesBlock({ activity }: { activity: ActivityResponse }) {
  const { t } = useTranslation();
  return (
    <div className="settings-diagnostics-route-list">
      <div className="settings-diagnostics-route-heading" aria-hidden="true">
        <span>{t("settings.diagnostics.detail.task")}</span>
        <span>{t("settings.diagnostics.detail.service")}</span>
        <span>{t("settings.diagnostics.detail.model")}</span>
        <span>{t("settings.diagnostics.detail.status")}</span>
      </div>
      <div className="settings-diagnostics-route-rows">
        {ROUTE_JOBS.map((job) => {
          const route = activity[job];
          const lane = knownLane(route.lane);
          return (
            <div key={job} data-testid={`diagnostics-route-${job}`} className="settings-diagnostics-route">
              <span className="settings-diagnostics-route-task">
                {t(`activityStatus.jobTitle.${job}`)}
              </span>
              <span className="settings-diagnostics-route-service">
                {lane === undefined ? t("settings.diagnostics.unknown") : t(laneKey(lane))}
              </span>
              <span className="settings-diagnostics-route-model">{route.model ?? t("settings.diagnostics.noModel")}</span>
              <StatusChip
                state={routeChipState(route)}
                label={route.available ? t("settings.diagnostics.available") : t("settings.diagnostics.notAvailable")}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}

const SUGGESTION_ACTION_KEYS = {
  entity_merge: "review.kindEntityMerge",
  detect_contradiction: "review.kindContradiction",
  dedup_merge: "review.kindDuplicate",
  relation_conflict: "review.kindRelationConflict",
  suggest_entity: "review.kindEntitySuggestion",
  page_merge: "review.kindPageMerge",
  cross_space_discovery: "review.kindCrossSpace",
  page_keep_or_archive: "review.kindPageArchive",
  lint_repair_review: "review.kindLintRepair",
  vocab_promote: "review.kindVocabPromote",
} as const satisfies Readonly<Record<string, string>>;

const SUGGESTION_STATUS_KEYS = {
  pending: "settings.diagnostics.suggestionPending",
  awaiting_review: "activity.refinementStatuses.awaitingReview",
  dismissed: "activity.refinementStatuses.dismissed",
  resolved: "activity.refinementStatuses.resolved",
  auto_applied: "activity.refinementStatuses.autoApplied",
  updated: "activity.refinementStatuses.updated",
} as const satisfies Readonly<Record<string, string>>;

function suggestionActionLabel(t: TFunction, action: string): string {
  const key = Object.prototype.hasOwnProperty.call(SUGGESTION_ACTION_KEYS, action)
    ? SUGGESTION_ACTION_KEYS[action as keyof typeof SUGGESTION_ACTION_KEYS]
    : undefined;
  return key ? t(key) : t("settings.diagnostics.unknownSuggestionAction", { value: action });
}

function suggestionStatusLabel(t: TFunction, status: string): string {
  const key = Object.prototype.hasOwnProperty.call(SUGGESTION_STATUS_KEYS, status)
    ? SUGGESTION_STATUS_KEYS[status as keyof typeof SUGGESTION_STATUS_KEYS]
    : undefined;
  return key ? t(key) : t("settings.diagnostics.unknownSuggestionStatus", { value: status });
}

function SuggestionsBlock({ activity }: { activity: ActivityResponse }) {
  const { t } = useTranslation();
  const { ready_for_review: ready, not_ready: notReady, groups } = activity.refinement;
  const headerStyle = {
    fontFamily: "var(--mem-font-body)",
    fontSize: "var(--mem-text-sm)",
    color: "var(--mem-text-secondary)",
    fontWeight: 400,
    textAlign: "left",
    paddingBottom: 4,
  } as const;
  return (
    <div data-testid="diagnostics-suggestions" className="settings-diagnostics-suggestions-content">
      <div className="settings-diagnostics-suggestion-counts">
        {(
          [
            ["ready", t("settings.diagnostics.readyForReview"), ready],
            ["not-ready", t("settings.diagnostics.notReady"), notReady],
          ] as const
        ).map(([id, label, count]) => (
          <div key={id} data-testid={`diagnostics-suggestions-${id}`} className="settings-diagnostics-suggestion-count">
            <span>{label}</span>
            <span>{count}</span>
          </div>
        ))}
      </div>
      {groups.length === 0 ? (
        <p className="settings-diagnostics-detail-empty">
          {t("settings.diagnostics.noOpenSuggestions")}
        </p>
      ) : (
        <div className="settings-diagnostics-suggestion-table-wrap">
          <table className="settings-diagnostics-suggestions w-full" style={{ borderCollapse: "collapse" }}>
            <thead>
              <tr>
                <th scope="col" style={headerStyle}>{t("settings.diagnostics.columnAction")}</th>
                <th scope="col" style={headerStyle}>{t("settings.diagnostics.columnStatus")}</th>
                <th scope="col" style={{ ...headerStyle, textAlign: "right" }}>{t("settings.diagnostics.columnCount")}</th>
              </tr>
            </thead>
            <tbody>
              {groups.map((group) => (
                <tr key={`${group.action}:${group.status}`} data-testid="diagnostics-suggestion-group">
                  <td title={group.action}>{suggestionActionLabel(t, group.action)}</td>
                  <td title={group.status}>{suggestionStatusLabel(t, group.status)}</td>
                  <td>{group.count}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function isUnreadable(value: Reading): boolean {
  return value.kind === "unreadable";
}

function integrationSummary(t: TFunction, wire: WireState): {
  state: ProbeState;
  label: string;
  messages: string[];
} {
  const messages: string[] = [];
  const binaryIncomplete =
    wire.mcp_binary.candidates.some((candidate) => candidate.state.kind === "unreadable") ||
    (wire.mcp_binary.undetermined?.length ?? 0) > 0;
  const unreadableClientRead = wire.clients.some((client) =>
    [client.detected, client.has_plugin, client.has_raw_entry, client.has_raw_duplicate].some(isUnreadable),
  );
  const duplicate = wire.clients.some(
    (client) =>
      (readingIsYes(client.has_plugin) && readingIsYes(client.has_raw_entry)) ||
      (readingIsNo(client.has_plugin) && readingIsYes(client.has_raw_duplicate)),
  );
  const unbound = wire.daemon.sidecar_job_binding?.state === "unbound";
  const stillRunning = wire.daemon.last_sidecar_stop?.outcome === "still_running";
  const unknownOwner = wire.daemon.sidecar_spawned_on_unknown_owner;
  const unverifiedStop = wire.daemon.last_sidecar_stop?.outcome === "could_not_measure";

  if (duplicate) messages.push(t("settings.diagnostics.overview.duplicateConfiguration"));
  if (binaryIncomplete) messages.push(t("settings.diagnostics.overview.binarySearchIncomplete"));
  if (unreadableClientRead) messages.push(t("settings.diagnostics.overview.clientCheckUnknown"));
  if (unbound) messages.push(t("settings.diagnostics.overview.sidecarUnbound"));
  if (stillRunning) messages.push(t("settings.diagnostics.overview.sidecarStillRunning"));
  if (unknownOwner) messages.push(t("settings.diagnostics.overview.sidecarOwnerUnknown"));
  if (unverifiedStop) messages.push(t("settings.diagnostics.overview.sidecarStopUnknown"));

  if (wire.mcp_binary.command === null) {
    messages.push(t("settings.diagnostics.overview.binaryMissing"));
    return {
      state: binaryIncomplete ? { kind: "unknown" } : { kind: "down" },
      label: binaryIncomplete
        ? t("settings.diagnostics.overview.unknown")
        : t("settings.diagnostics.overview.needsAttention"),
      messages,
    };
  }
  if (duplicate || unbound || stillRunning) {
    return { state: { kind: "down" }, label: t("settings.diagnostics.overview.needsAttention"), messages };
  }
  if (binaryIncomplete || unreadableClientRead || unknownOwner || unverifiedStop) {
    return { state: { kind: "unknown" }, label: t("settings.diagnostics.overview.unknown"), messages };
  }
  const toolDetected = wire.clients.some((client) => readingIsYes(client.detected));
  return toolDetected
    ? { state: { kind: "up" }, label: t("settings.diagnostics.overview.toolDetected"), messages }
    : { state: { kind: "idle" }, label: t("settings.diagnostics.overview.noToolDetected"), messages };
}

function activityOverview(t: TFunction, activity: ActivityResponse): {
  state: ProbeState;
  label: string;
  messages: string[];
  needsIntelligence: boolean;
} {
  const state = knownState(activity);
  const failed = activity.assets.some((asset) => asset.steps.some((step) => step.failed > 0));
  const blocked = activity.assets.some((asset) => asset.state === "blocked" || asset.blocked > 0);
  const hasUnknownDetails = activity.assets.some((asset) =>
    !isKnownKind(asset.kind) ||
    !["idle", "running", "blocked"].includes(asset.state) ||
    asset.steps.some((step) => !isKnownStep(step) || !["idle", "running", "blocked"].includes(step.state)),
  );
  const everydayUnavailable = activity.everyday.mode === "pinned_unavailable";
  const synthesisUnavailable = activity.synthesis.mode === "pinned_unavailable";
  const hasUnknownRoute =
    !knownLane(activity.everyday.lane) ||
    !knownLane(activity.synthesis.lane) ||
    !["auto", "pinned", "pinned_unavailable", "unconfigured"].includes(activity.everyday.mode) ||
    !["auto", "pinned", "pinned_unavailable", "unconfigured"].includes(activity.synthesis.mode);
  // On demand (`off`) runs no background model, so pending work and an
  // unavailable model are not faults here: the user's AI tool does the work.
  // It says what is waiting instead, the same as Activity.
  const onDemand = state === "off";
  const pagesWaiting = onDemandPagesWaiting(activity);
  const messages: string[] = [];
  if (failed) messages.push(t("settings.diagnostics.overview.backgroundFailed"));
  if (blocked && !onDemand) messages.push(t("settings.diagnostics.overview.backgroundBlocked"));
  if (everydayUnavailable && !onDemand) messages.push(t("settings.diagnostics.overview.everydayUnavailable"));
  if (synthesisUnavailable && !onDemand) messages.push(t("settings.diagnostics.overview.synthesisUnavailable"));
  if (pagesWaiting > 0) messages.push(t("activityStatus.assetOnDemand.pages", { count: pagesWaiting }));
  if (hasUnknownDetails || hasUnknownRoute || state === undefined) messages.push(t("settings.diagnostics.overview.backgroundUnknown"));

  let probe: ProbeState = { kind: "up" };
  if (state === undefined) probe = { kind: "unknown" };
  else if (failed) probe = { kind: "down" };
  else if (onDemand) probe = { kind: "idle" };
  else if (state === "blocked" || blocked || everydayUnavailable || synthesisUnavailable) probe = { kind: "down" };
  else if (hasUnknownDetails || hasUnknownRoute) probe = { kind: "unknown" };

  return {
    state: probe,
    label: state === undefined ? t("settings.diagnostics.overview.unknown") : t(`activityStatus.state.${state}`),
    messages,
    needsIntelligence: !onDemand && (everydayUnavailable || synthesisUnavailable),
  };
}

function ActivitySummary({
  activity,
  onOpenActivity,
  onOpenIntelligence,
}: {
  activity: ActivityResponse;
  onOpenActivity?: () => void;
  onOpenIntelligence?: () => void;
}) {
  const { t, i18n } = useTranslation();
  const summary = activityOverview(t, activity);
  const lastActivity = activity.last_activity_at === null
    ? t("activityStatus.neverActive")
    : t("activityStatus.lastActivity", {
        time: relativeTime(activity.last_activity_at, t, i18n.language),
      });
  return (
    <div data-testid="diagnostics-activity-summary" className="settings-diagnostics-activity-summary">
      <div className="settings-diagnostics-activity-summary-main">
        <span data-testid="diagnostics-state"><StatusChip state={summary.state} label={summary.label} /></span>
        <span data-testid="diagnostics-last-activity">{lastActivity}</span>
      </div>
      {summary.messages.length > 0 && (
        <ul className="settings-diagnostics-activity-warnings">
          {summary.messages.map((message) => <li key={message}>{message}</li>)}
        </ul>
      )}
      {(onOpenActivity || (summary.needsIntelligence && onOpenIntelligence)) && (
        <div className="settings-diagnostics-activity-actions">
          {onOpenActivity && (
            <Button variant="ghost" size="sm" onClick={onOpenActivity}>
              {t("settings.diagnostics.overview.openActivity")}
            </Button>
          )}
          {summary.needsIntelligence && onOpenIntelligence && (
            <Button variant="ghost" size="sm" onClick={onOpenIntelligence}>
              {t("settings.diagnostics.overview.openIntelligence")}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}

function OverviewRow({
  title,
  state,
  label,
  messages = [],
  actions,
  testId,
}: {
  title: string;
  state: ProbeState;
  label: string;
  messages?: string[];
  actions?: ReactNode;
  testId: string;
}) {
  return (
    <div className="settings-diagnostics-overview-row" data-testid={testId}>
      <div className="settings-diagnostics-overview-main">
        <span className="settings-diagnostics-overview-title">{title}</span>
        <StatusChip state={state} label={label} />
      </div>
      {(messages.length > 0 || actions) && (
        <div className="settings-diagnostics-overview-note">
          {messages.map((message, index) => <span key={`${message}:${index}`}>{message}</span>)}
          {actions}
        </div>
      )}
    </div>
  );
}

function DiagnosticsOverview({
  wireQuery,
  activityQuery,
  onOpenConnections,
  onOpenIntelligence,
  onOpenActivity,
}: {
  wireQuery: UseQueryResult<WireState>;
  activityQuery: UseQueryResult<ActivityResponse>;
  onOpenConnections?: () => void;
  onOpenIntelligence?: () => void;
  onOpenActivity?: () => void;
}) {
  const { t } = useTranslation();
  let serviceState: ProbeState = { kind: "probing" };
  let serviceLabel = t("settings.diagnostics.overview.checking");
  let serviceMessage: string[] = [];
  if (wireQuery.isError) {
    serviceState = { kind: "unknown" };
    serviceLabel = t("settings.diagnostics.overview.unknown");
    serviceMessage = [t("settings.diagnostics.overview.serviceCheckUnknown")];
  } else if (wireQuery.data) {
    serviceState = wireQuery.data.daemon.reachable ? { kind: "up" } : { kind: "down" };
    serviceLabel = wireQuery.data.daemon.reachable
      ? t("settings.diagnostics.overview.responding")
      : t("settings.diagnostics.overview.notResponding");
    if (!wireQuery.data.daemon.reachable) serviceMessage = [t("settings.diagnostics.overview.serviceNotResponding")];
  }

  let integrationState: ProbeState = { kind: "probing" };
  let integrationLabel = t("settings.diagnostics.overview.checking");
  let integrationMessages: string[] = [];
  if (wireQuery.isError) {
    integrationState = { kind: "unknown" };
    integrationLabel = t("settings.diagnostics.overview.unknown");
    integrationMessages = [t("settings.diagnostics.overview.integrationCheckUnknown")];
  } else if (wireQuery.data) {
    const summary = integrationSummary(t, wireQuery.data);
    integrationState = summary.state;
    integrationLabel = summary.label;
    integrationMessages = summary.messages;
  }

  let backgroundState: ProbeState = { kind: "probing" };
  let backgroundLabel = t("settings.diagnostics.overview.checking");
  let backgroundMessages: string[] = [];
  let needsIntelligence = false;
  if (activityQuery.isError) {
    backgroundState = { kind: "unknown" };
    backgroundLabel = t("settings.diagnostics.overview.unknown");
    backgroundMessages = [t("settings.diagnostics.overview.backgroundCheckUnknown")];
  } else if (activityQuery.data) {
    const summary = activityOverview(t, activityQuery.data);
    backgroundState = summary.state;
    backgroundLabel = summary.label;
    backgroundMessages = summary.messages;
    needsIntelligence = summary.needsIntelligence;
  }

  const wireIssue = wireQuery.isError || Boolean(wireQuery.data && (!wireQuery.data.daemon.reachable || integrationState.kind === "down" || integrationState.kind === "unknown"));
  return (
    <div className="settings-diagnostics-overview" data-testid="diagnostics-overview">
      <OverviewRow
        testId="diagnostics-overview-service"
        title={t("settings.diagnostics.overview.localService")}
        state={serviceState}
        label={serviceLabel}
        messages={serviceMessage}
      />
      <OverviewRow
        testId="diagnostics-overview-integration"
        title={t("settings.diagnostics.overview.aiIntegration")}
        state={integrationState}
        label={integrationLabel}
        messages={integrationMessages}
        actions={wireIssue && onOpenConnections ? <Button variant="ghost" size="sm" onClick={onOpenConnections}>{t("settings.diagnostics.overview.openConnections")}</Button> : undefined}
      />
      <OverviewRow
        testId="diagnostics-overview-background"
        title={t("settings.diagnostics.overview.backgroundProcessing")}
        state={backgroundState}
        label={backgroundLabel}
        messages={backgroundMessages}
        actions={onOpenActivity || (needsIntelligence && onOpenIntelligence) ? (
          <>
            {onOpenActivity && <Button variant="ghost" size="sm" onClick={onOpenActivity}>{t("settings.diagnostics.overview.openActivity")}</Button>}
            {needsIntelligence && onOpenIntelligence && <Button variant="ghost" size="sm" onClick={onOpenIntelligence}>{t("settings.diagnostics.overview.openIntelligence")}</Button>}
          </>
        ) : undefined}
      />
    </div>
  );
}

function DiagnosticsError({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  const { t } = useTranslation();
  // Version skew (an old daemon lacking the activity route) is not a failure —
  // it reads as a warning (amber), and retrying can't fix it, so no Retry.
  // A genuine unavailability keeps the danger tone and offers a Retry.
  const versionSkew = isOldDaemonError(error);
  return (
    <div className="px-5 py-4">
      <p
        style={{
          fontFamily: "var(--mem-font-body)",
          fontSize: "var(--mem-text-sm)",
          color: versionSkew ? "var(--mem-status-warning-text)" : "var(--mem-status-danger-text)",
          lineHeight: "1.5",
        }}
      >
        {versionSkew ? t("settings.diagnostics.needsNewerDaemon") : t("settings.diagnostics.unavailable")}
      </p>
      {!versionSkew && (
        <div className="mt-3">
          <Button variant="secondary" size="sm" onClick={onRetry}>
            {t("settings.diagnostics.wiring.retry")}
          </Button>
        </div>
      )}
    </div>
  );
}

export default function DiagnosticsSection({
  onSetupAgent,
  onOpenConnections,
  onOpenIntelligence,
  onOpenActivity,
}: {
  onSetupAgent?: () => void;
  onOpenConnections?: () => void;
  onOpenIntelligence?: () => void;
  onOpenActivity?: () => void;
}) {
  const { t } = useTranslation();
  const [showDetails, setShowDetails] = useState(false);
  const wireQuery = useQuery({
    queryKey: ["wireState"],
    queryFn: getWireState,
    retry: false,
  });
  const activityQuery = useActivity();
  const refreshChecks = () => {
    void wireQuery.refetch();
    void activityQuery.refetch();
  };

  return (
    <div className="settings-diagnostics">
      <section className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SectionHeader
          label={showDetails ? t("settings.diagnostics.overview.technicalDetails") : t("settings.diagnostics.overview.title")}
          action={
            <div className="settings-diagnostics-header-actions">
              <Button variant="secondary" size="sm" onClick={refreshChecks}>{t("settings.diagnostics.overview.refreshChecks")}</Button>
              <Button variant="secondary" size="sm" data-testid="diagnostics-details-toggle" onClick={() => setShowDetails((value) => !value)}>
                {showDetails ? t("settings.diagnostics.overview.backToOverview") : t("settings.diagnostics.overview.technicalDetails")}
              </Button>
            </div>
          }
        />
        {!showDetails ? (
          <Card padding="rows">
            <DiagnosticsOverview
              wireQuery={wireQuery}
              activityQuery={activityQuery}
              onOpenConnections={onOpenConnections}
              onOpenIntelligence={onOpenIntelligence}
              onOpenActivity={onOpenActivity}
            />
          </Card>
        ) : (
          <div className="settings-diagnostics-details">
            {wireQuery.isError && wireQuery.data && <StaleSnapshot error={wireQuery.error} />}
            {activityQuery.isError && activityQuery.data && <StaleSnapshot error={activityQuery.error} />}
            <DetailSection title={t("settings.diagnostics.wiring.daemonTitle")}>
              {wireQuery.isLoading && <WireLoading announce />}
              {wireQuery.isError && <WiringError onRetry={() => wireQuery.refetch()} />}
              {wireQuery.data && <DaemonStatus daemon={wireQuery.data.daemon} onRetry={() => wireQuery.refetch()} />}
            </DetailSection>
            <DetailSection title={t("settings.diagnostics.wiring.mcpBinaryTitle")}>
              {wireQuery.isLoading && <WireLoading />}
              {wireQuery.isError && <WireUnavailable />}
              {wireQuery.data && <McpBinaryStatus mcpBinary={wireQuery.data.mcp_binary} onSetupAgent={onSetupAgent} />}
            </DetailSection>
            <DetailSection title={t("settings.diagnostics.overview.aiIntegration")}>
              {wireQuery.isLoading && <WireLoading />}
              {wireQuery.isError && <WireUnavailable />}
              {wireQuery.data && <ClientsWiring clients={wireQuery.data.clients} />}
            </DetailSection>
            <DetailSection title={t("settings.diagnostics.backgroundTitle")}>
              {activityQuery.isLoading && <ActivityPending loading />}
              {activityQuery.isError && <DiagnosticsError error={activityQuery.error} onRetry={() => activityQuery.refetch()} />}
              {activityQuery.data && (
                <ActivitySummary
                  activity={activityQuery.data}
                  onOpenActivity={onOpenActivity}
                  onOpenIntelligence={onOpenIntelligence}
                />
              )}
            </DetailSection>
            <DetailSection title={t("settings.diagnostics.routesTitle")}>
              {activityQuery.isLoading && <ActivityPending loading />}
              {activityQuery.isError && <ActivityPending loading={false} />}
              {activityQuery.data && <RoutesBlock activity={activityQuery.data} />}
            </DetailSection>
            <DetailSection title={t("activityDiscoveries.title")}>
              {activityQuery.isLoading && <ActivityPending loading />}
              {activityQuery.isError && <ActivityPending loading={false} />}
              {activityQuery.data && <SuggestionsBlock activity={activityQuery.data} />}
            </DetailSection>
          </div>
        )}
      </section>
    </div>
  );
}
