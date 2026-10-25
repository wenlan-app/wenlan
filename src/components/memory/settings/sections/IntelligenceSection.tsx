// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  getBackgroundAiEnabled,
  getExternalLlm,
  getLockMyPages,
  getOnDeviceModel,
  getResolvedRouting,
  setBackgroundAiEnabled,
  setLockMyPages,
  setSourcePin,
} from "../../../../lib/tauri";
import AnyProviderCard from "../../../intelligence/AnyProviderCard";
import {
  OnDeviceModelManager,
  RoutineModelSelect,
  SynthesisModelSelect,
  useApiKeyStatus,
} from "../../../intelligence/IntelligenceSetup";
import { presetForEndpoint, type PresetGroup } from "../../../intelligence/providerPresets";
import { fillUnsetPinsWithRetry } from "../../../../lib/routingPins";
import { Button, Card, Select, SectionHeader, SegmentedControl, SettingRow } from "../primitives";
import { SettingsGroup } from "../SettingsView";
import { SettingsHelp } from "../SettingsHelp";

// Module-level so each is a stable reference across renders — AnyProviderCard
// memoizes its preset list on these (same convention as SetupWizard's
// identical CLOUD_GROUPS/LOCAL_GROUPS, whose steps these rows mirror).
const CLOUD_GROUPS: PresetGroup[] = ["cloud"];
const LOCAL_GROUPS: PresetGroup[] = ["local", "custom"];

type JobRowId = "everyday" | "synthesis";
type SourceRowId = "cloud" | "local" | "onDevice";
type PinSource = "anthropic" | "external" | "on_device";
const metaLineStyle = { fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)", lineHeight: 1.5 } as const;
const amberStyle = { ...metaLineStyle, color: "var(--mem-status-warning-text)" } as const;
function isPinSource(source: string | null): source is PinSource {
  return source === "anthropic" || source === "external" || source === "on_device";
}
interface SourceOption { source: PinSource; label: string; available: boolean; }
interface JobView {
  job: JobRowId;
  source: string;
  sourceDisplay: string;
  /** Configured preference, kept separate from the route's actual state. */
  selectedSource: PinSource | null;
  connected: boolean;
  degraded: boolean;
  pinnedDisplay: string | null;
  sourceOptions: SourceOption[];
}

/** Shared models belong to services; only Anthropic's independent task model
 * gets an additional control. The service choice is always directly editable. */
function TaskRow({ name, nameAddon, view, isPinned, loading, onPickSource, onSetUp }: {
  name: string;
  nameAddon?: React.ReactNode;
  view: JobView;
  isPinned: boolean;
  loading: boolean;
  onPickSource: (source: PinSource) => Promise<void>;
  onSetUp: () => void;
}) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState(false);
  const { job, source, selectedSource, sourceDisplay, degraded, pinnedDisplay, sourceOptions } = view;
  const unassignedLabel = t(job === "everyday" ? "intelligence.taskBasicStatus" : "intelligence.taskOffStatus");
  const noServiceAvailable = sourceOptions.every((option) => !option.available);
  const modelSource = selectedSource ?? (!degraded && isPinSource(source) ? source : null);
  return (
    <div className="settings-task-row settings-row px-5 py-4" data-task={job}>
      <div className="settings-task-field">
        <div className="settings-row-title-line flex min-w-0 items-center gap-1.5">
          <div style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-md)", fontWeight: 500, color: "var(--mem-text)" }}>{name}</div>
          {nameAddon}
        </div>
        <div className="settings-task-control">
          {isPinned ? (
            <Select size="sm" aria-label={t("intelligence.chooseTaskService", { task: name })} value={selectedSource ?? ""} disabled={loading || saving}
              onChange={async (event) => {
                const next = event.target.value;
                if (!isPinSource(next)) return;
                setSaving(true); setSaveError(false);
                try { await onPickSource(next); }
                catch { setSaveError(true); }
                finally { setSaving(false); }
              }}>
              {selectedSource === null && <option value="" disabled>{degraded ? t("settings.controlState.unavailable") : unassignedLabel}</option>}
              {sourceOptions.map((option) => (
                <option key={option.source} value={option.source} disabled={!option.available}>
                  {option.available || option.source === selectedSource ? option.label : `${option.label}${t("intelligence.sourceUnavailableSuffix")}`}
                </option>
              ))}
            </Select>
          ) : (
            <div className="settings-task-value" style={metaLineStyle}>{loading ? t("settings.controlState.loading") : isPinSource(source) ? sourceDisplay : unassignedLabel}</div>
          )}
        </div>
      </div>
      {modelSource === "anthropic" && <div className="settings-task-model">{job === "everyday" ? <RoutineModelSelect compact /> : <SynthesisModelSelect compact />}</div>}
      {degraded && <p className="settings-task-message" role="status" style={amberStyle}>{t(pinnedDisplay ? "intelligence.pinnedDegradedHintPinned" : "intelligence.pinnedDegradedHint", { pinned: pinnedDisplay })}</p>}
      {saveError && <p className="settings-task-message" role="alert" style={{ ...metaLineStyle, color: "var(--mem-status-danger-text)" }}>{t("intelligence.taskServiceSaveError")}</p>}
      {!loading && (noServiceAvailable || selectedSource === null) && !degraded && <div className="settings-task-setup"><Button variant="ghost" size="sm" onClick={onSetUp}>{t("intelligence.setUpService")}</Button></div>}
    </div>
  );
}

/** Where an external endpoint sends text: a vendor's name, its host, or null
 * when it is a server on this computer. A cloud preset names its vendor; a
 * custom endpoint is local only when its host is this machine. */
function externalVendor(endpoint: string): string | null {
  const preset = presetForEndpoint(endpoint);
  if (preset.group === "cloud") return preset.name;
  if (preset.group === "local") return null;
  try {
    const host = new URL(endpoint).hostname;
    return host === "localhost" || host === "[::1]" || host === "::1" || host.startsWith("127.") ? null : host;
  } catch {
    return endpoint;
  }
}

/** Says whether steeping in Automatic sends text off this computer, and which
 * work does. Built from the same routes the task rows show. */
function SteepTrustRow({ jobs, externalEndpoint }: {
  jobs: { name: string; source: string | null }[];
  externalEndpoint: string | null;
}) {
  const { t } = useTranslation();
  const sending: string[] = [];
  const vendors: string[] = [];
  for (const job of jobs) {
    const vendor = job.source === "anthropic"
      ? t("intelligence.sourceAnthropic")
      : job.source === "external" && externalEndpoint
        ? externalVendor(externalEndpoint)
        : null;
    if (vendor === null) continue;
    sending.push(job.name);
    if (!vendors.includes(vendor)) vendors.push(vendor);
  }
  const separator = t("activityStatus.jobSeparator");
  return (
    <p className="settings-row settings-steep-trust px-5 py-3" data-testid="steep-trust" style={metaLineStyle}>
      {sending.length === 0
        ? t("intelligence.steepTrustLocal")
        : t("intelligence.steepTrustCloud", { count: sending.length, jobs: sending.join(separator), vendor: vendors.join(separator) })}
    </p>
  );
}

/** The steeping mode: Automatic (`background_ai_enabled` true) or On demand
 * (false). Both are steeping; neither is "off". */
function SteepModeRow({ automatic, loading, saving, readiness, error, onChoose }: {
  automatic: boolean | undefined;
  loading: boolean;
  saving: boolean;
  readiness?: string;
  error: string | null;
  onChoose: (automatic: boolean) => Promise<void>;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const name = useId();
  const disabled = automatic === undefined || saving;
  const options = [
    { automatic: true, id: "automatic", label: t("intelligence.steepModeAutomatic"), hint: t("intelligence.steepModeAutomaticHint") },
    { automatic: false, id: "on-demand", label: t("intelligence.steepModeOnDemand"), hint: t("intelligence.steepModeOnDemandHint") },
  ];
  return (
    <div className="settings-row settings-steep-mode px-5 py-4" data-testid="steep-mode">
      <div className="settings-row-title-line flex min-w-0 items-center gap-1.5">
        <div id={titleId} style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-md)", fontWeight: 500, color: "var(--mem-text)" }}>{t("intelligence.backgroundAiTitle")}</div>
        <SettingsHelp title={t("intelligence.backgroundAiTitle")} details={t("intelligence.backgroundAiDescription")} />
      </div>
      <div role="radiogroup" aria-labelledby={titleId} aria-busy={saving || loading} className="settings-steep-mode-options">
        {options.map((option) => (
          <label key={option.id} className="settings-steep-mode-option" data-selected={automatic === option.automatic} aria-disabled={disabled}>
            <input type="radio" className="settings-steep-mode-radio" name={name} value={option.id}
              checked={automatic === option.automatic} disabled={disabled}
              aria-labelledby={`${name}-${option.id}-label`}
              aria-describedby={`${name}-${option.id}-hint`}
              onChange={() => void onChoose(option.automatic)} />
            <span className="settings-steep-mode-copy">
              <span id={`${name}-${option.id}-label`} className="settings-steep-mode-title">{option.label}</span>
              <span id={`${name}-${option.id}-hint`} className="settings-steep-mode-hint">{option.hint}</span>
            </span>
          </label>
        ))}
      </div>
      {automatic === undefined && !error && (
        <p style={metaLineStyle}>{t(loading ? "settings.controlState.loading" : saving ? "settings.controlState.saving" : "settings.controlState.unavailable")}</p>
      )}
      {automatic === true && readiness && <p className="settings-steep-mode-note" role="status" style={amberStyle}>{readiness}</p>}
      <p className="settings-steep-mode-note" style={metaLineStyle}>{t("intelligence.steepCheckNote")}</p>
      {error && <p role="alert" style={{ ...metaLineStyle, color: "var(--mem-status-danger-text)" }}>{error}</p>}
    </div>
  );
}

/** "Ask me first on pages I wrote": AI changes to pages the user wrote or edited
 * wait for review. Switching it on also locks the pages they already wrote and
 * says how many in one grey line; switching it off unlocks nothing, and says that. */
function LockMyPagesRow() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: ["lockMyPages"], queryFn: getLockMyPages });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(false);
  // What the last switch did: how many pages it set to Ask me first, or "off"
  // (which leaves those pages as they are, so the line says so).
  const [changed, setChanged] = useState<number | "off" | null>(null);
  return (
    <div data-testid="lock-my-pages">
      <SettingRow
        title={t("intelligence.lockMyPages")}
        description={t("intelligence.lockMyPagesHint")}
        enabled={query.data === true}
        valueUnknown={!query.isSuccess || saving}
        unknownState={saving ? "saving" : query.isPending ? "loading" : "unavailable"}
        toggleHitArea
        error={error ? t("intelligence.backgroundAiSaveError") : null}
        statusLine={changed !== null
          ? <p role="status" style={metaLineStyle}>{changed === "off"
            ? t("intelligence.lockMyPagesOff")
            : changed === 0 ? t("intelligence.lockMyPagesNone") : t("intelligence.lockMyPagesLocked", { count: changed })}</p>
          : undefined}
        onToggle={async () => {
          if (!query.isSuccess || saving) return;
          const next = !query.data;
          setSaving(true);
          setError(false);
          try {
            const count = await setLockMyPages(next);
            setChanged(next ? count ?? 0 : "off");
            queryClient.setQueryData(["lockMyPages"], next);
            queryClient.invalidateQueries({ queryKey: ["page"] });
            queryClient.invalidateQueries({ queryKey: ["pages"] });
          } catch {
            setError(true);
          } finally {
            setSaving(false);
          }
        }}
      />
    </div>
  );
}

export default function IntelligenceSection({ delay }: { delay: number }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const backgroundAi = useQuery({ queryKey: ["backgroundAiEnabled"], queryFn: getBackgroundAiEnabled });
  const [savingBackgroundAi, setSavingBackgroundAi] = useState(false);
  const [backgroundAiError, setBackgroundAiError] = useState(false);
  const backgroundAiValue = backgroundAi.isSuccess ? backgroundAi.data : undefined;
  const anthropic = useApiKeyStatus();
  const { data: external } = useQuery({ queryKey: ["external-llm"], queryFn: getExternalLlm });
  const { data: onDevice } = useQuery({ queryKey: ["onDeviceModel"], queryFn: getOnDeviceModel });
  // Feature detection: null ⇒ the daemon has no routing endpoint (LEGACY mode);
  // an object ⇒ per-job pins are live (PINNED mode). undefined while loading —
  // treated as legacy, which is also what the live 0.13.2 daemon returns.
  const routingQuery = useQuery({ queryKey: ["resolvedRouting"], queryFn: getResolvedRouting });
  const routing = routingQuery.data;
  const isPinned = routing != null;

  // The daemon holds exactly one external-llm slot — a saved local server and
  // a saved cloud vendor overwrite each other — so the saved slot is
  // attributed to exactly one row, via its own preset's group.
  const [externalEndpoint, externalModel] = external ?? [null, null];
  const externalPreset = externalEndpoint ? presetForEndpoint(externalEndpoint) : null;
  const externalConfigured = externalPreset !== null;

  const loadedId = onDevice?.loaded ?? null;
  const onDeviceLoaded = !!loadedId;

  // Provider slot the app can attribute an external source to, in either mode.
  const routedExternal = isPinned
    ? routing.pool.external
    : externalEndpoint && externalModel
      ? { endpoint: externalEndpoint, model: externalModel }
      : null;
  const providerName = routedExternal ? presetForEndpoint(routedExternal.endpoint)?.name ?? null : null;

  // What each provider source can currently serve — drives option enable/disable
  // in the pinned source select and the legacy derived chain.
  const avail = isPinned
    ? {
        anthropic: routing.pool.anthropic.configured,
        external: routing.pool.external != null,
        on_device: routing.pool.on_device?.loaded === true,
      }
    : { anthropic: anthropic.isConfigured, external: externalConfigured, on_device: onDeviceLoaded };

  const sourceLabel = (source: string): string => {
    switch (source) {
      case "anthropic":
        return t("intelligence.sourceAnthropic");
      case "external":
        return providerName ?? t("intelligence.sourceConnectedProvider");
      case "on_device":
        return t("intelligence.sourceOnDevice");
      case "basic":
        return t("intelligence.servingBasic");
      default:
        return t("intelligence.synthesisNoSourceHint");
    }
  };

  const buildOptions = (): SourceOption[] => {
    // Keep service choices in the same order for both tasks.
    const order: PinSource[] = ["on_device", "anthropic", "external"];
    return order.map((s) => {
      const available = avail[s];
      const base =
        s === "anthropic"
          ? t("intelligence.sourceAnthropic")
          : s === "on_device"
            ? t("intelligence.sourceOnDevice")
            : providerName ?? t("intelligence.sourceConnectedProvider");
      return { source: s, label: base, available };
    });
  };

  const buildView = (job: JobRowId): JobView => {
    const source = isPinned
      ? (job === "everyday" ? routing.everyday.source : routing.synthesis.source)
      : job === "everyday"
        ? anthropic.isConfigured
          ? "anthropic"
          : onDeviceLoaded
            ? "on_device"
            : "basic"
        : anthropic.isConfigured
          ? "anthropic"
          : externalConfigured
            ? "external"
            : "none";
    const mode = isPinned ? (job === "everyday" ? routing.everyday.mode : routing.synthesis.mode) : "legacy";
    const pin = isPinned ? (job === "everyday" ? routing.everyday.pin : routing.synthesis.pin) : null;
    return {
      job,
      source,
      sourceDisplay: sourceLabel(source),
      selectedSource: isPinSource(pin) ? pin : mode !== "pinned_unavailable" && isPinSource(source) ? source : null,
      connected: source !== "basic" && source !== "none",
      degraded: mode === "pinned_unavailable",
      pinnedDisplay: pin ? sourceLabel(pin) : null,
      sourceOptions: buildOptions(),
    };
  };

  const everydayView = buildView("everyday");
  const synthesisView = buildView("synthesis");

  const pickSource = async (job: JobRowId, source: PinSource) => {
    await setSourcePin(job === "everyday" ? source : null, job === "synthesis" ? source : null);
    await queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] });
    queryClient.invalidateQueries({ queryKey: ["activity"] });
  };

  const hasAvailableAssignment = [everydayView, synthesisView].every((view) => view.connected && !view.degraded);
  const backgroundReadiness = backgroundAi.isSuccess && routingQuery.isSuccess && backgroundAiValue && !hasAvailableAssignment
    ? t("intelligence.backgroundSetupRequired")
    : undefined;
  const servicesRef = useRef<HTMLDivElement>(null);
  const [activeService, setActiveService] = useState<SourceRowId>("cloud");
  const [visitedServices, setVisitedServices] = useState<Set<SourceRowId>>(() => new Set(["cloud"]));
  const activateService = (id: SourceRowId) => {
    setVisitedServices((visited) => new Set(visited).add(id));
    setActiveService(id);
  };
  const setUpService = (job: JobRowId) => {
    activateService(job === "everyday" ? "onDevice" : "cloud");
    servicesRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  return (
    <section className="mem-fade-up settings-intelligence" style={{ animationDelay: `${delay}ms` }}>
      <div className="flex flex-col gap-6">
        <SettingsGroup>
          <SteepModeRow
            automatic={backgroundAiValue}
            loading={backgroundAi.isPending}
            saving={savingBackgroundAi}
            readiness={backgroundReadiness}
            error={backgroundAiError || backgroundAi.isError ? t("intelligence.backgroundAiSaveError") : null}
            onChoose={async (enabled) => {
              if (!backgroundAi.isSuccess || backgroundAiValue === undefined || savingBackgroundAi || enabled === backgroundAiValue) return;
              setSavingBackgroundAi(true);
              setBackgroundAiError(false);
              try {
                await setBackgroundAiEnabled(enabled);
                if (enabled) await fillUnsetPinsWithRetry();
                await Promise.all([
                  queryClient.invalidateQueries({ queryKey: ["backgroundAiEnabled"] }),
                  queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] }),
                  queryClient.invalidateQueries({ queryKey: ["activity"] }),
                ]);
              } catch {
                setBackgroundAiError(true);
              } finally {
                setSavingBackgroundAi(false);
              }
            }}
          />
          <LockMyPagesRow />
          {backgroundAiValue === true && (
            <>
              <TaskRow
                name={t("intelligence.everydayWork")}
                nameAddon={<SettingsHelp title={t("intelligence.everydayWork")} details={`${t("intelligence.routineModelTipBody")}${!isPinned && !routingQuery.isPending ? ` ${t("intelligence.sourcePinLegacyHint")}` : ""}`} />}
                view={everydayView} isPinned={isPinned} loading={routingQuery.isPending}
                onPickSource={(source) => pickSource("everyday", source)} onSetUp={() => setUpService("everyday")}
              />
              <TaskRow
                name={t("intelligence.synthesisWork")}
                nameAddon={<span className="inline-flex items-center gap-2">
                  <span className="rounded px-1.5 py-0.5" style={{ fontSize: "var(--mem-text-2xs)", color: "var(--mem-text-secondary)", backgroundColor: "var(--mem-hover)" }}>{t("intelligence.synthesisBeta")}</span>
                  <SettingsHelp title={t("intelligence.synthesisBeta")} details={`${t("intelligence.synthesisBetaHelp")}${!isPinned && !routingQuery.isPending ? ` ${t("intelligence.sourcePinLegacyHint")}` : ""}`} />
                </span>}
                view={synthesisView} isPinned={isPinned} loading={routingQuery.isPending}
                onPickSource={(source) => pickSource("synthesis", source)} onSetUp={() => setUpService("synthesis")}
              />
              <SteepTrustRow jobs={[
                { name: t("intelligence.everydayWork"), source: everydayView.connected && !everydayView.degraded ? everydayView.source : null },
                { name: t("intelligence.synthesisWork"), source: synthesisView.connected && !synthesisView.degraded ? synthesisView.source : null },
              ]} externalEndpoint={routedExternal?.endpoint ?? null} />
            </>
          )}
        </SettingsGroup>
        <div ref={servicesRef} className="mem-fade-up" style={{ animationDelay: `${delay + 30}ms` }}>
          <SectionHeader
            label={t("intelligence.aiServices")}
            help={<SettingsHelp title={t("intelligence.servicesHelpTitle")} details={`${t("intelligence.servicesHelpBody")} ${t("intelligence.modelsSharedNotice")}`} />}
          />
          <Card padding="none">
            <div className="mx-5 mt-4 mb-3">
              <SegmentedControl<SourceRowId>
                aria-label={t("intelligence.aiServices")}
                size="md"
                tabPanelId="intelligence-service-panel"
                tabIdPrefix="intelligence-service-tab"
                options={[
                  { value: "cloud", label: t("intelligence.cloudTab") },
                  { value: "onDevice", label: t("intelligence.deviceTab") },
                  { value: "local", label: t("intelligence.localTab") },
                ]}
                value={activeService}
                onChange={activateService}
              />
            </div>
            <div
              role="tabpanel"
              id="intelligence-service-panel"
              aria-labelledby={`intelligence-service-tab-${activeService}`}
              className="px-5 pb-5"
            >
              {visitedServices.has("cloud") && (
                <div hidden={activeService !== "cloud"}>
                  <AnyProviderCard bare settingsPresentation groups={CLOUD_GROUPS} showTaskModels={false} />
                </div>
              )}
              {visitedServices.has("local") && (
                <div hidden={activeService !== "local"}>
                  <AnyProviderCard bare settingsPresentation groups={LOCAL_GROUPS} showTaskModels={false} />
                </div>
              )}
              {visitedServices.has("onDevice") && (
                <div hidden={activeService !== "onDevice"} className="flex flex-col gap-3">
                  <OnDeviceModelManager />
                </div>
              )}
            </div>
          </Card>
        </div>
      </div>
    </section>
  );
}
