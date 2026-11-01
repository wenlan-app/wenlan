import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as shellOpen } from "@tauri-apps/plugin-shell";
import {
  downloadOnDeviceModel,
  getApiKey,
  getModelChoice,
  getOnDeviceModel,
  getSystemInfo,
  setApiKey,
  setModelChoice,
  type OnDeviceModelEntry,
} from "../../lib/tauri";
import { fillUnsetPinsAfterSave } from "../../lib/routingPins";
import { ACTIVITY_QUERY_KEY } from "../../lib/useActivity";
import { Card, Field, Input, Button, Select, StatusChip } from "../memory/settings/primitives";
import { SettingsHelp } from "../memory/settings/SettingsHelp";

type AnthropicModelDescriptionKey =
  | "intelligence.modelDescriptions.fastAffordable"
  | "intelligence.modelDescriptions.balancedQuality"
  | "intelligence.modelDescriptions.maximumQuality";

export const ANTHROPIC_MODELS = [
  { id: "claude-haiku-4-5-20251001", label: "Haiku 4.5", descKey: "intelligence.modelDescriptions.fastAffordable" },
  { id: "claude-sonnet-4-6", label: "Sonnet 4.6", descKey: "intelligence.modelDescriptions.balancedQuality" },
  { id: "claude-opus-4-6", label: "Opus 4.6", descKey: "intelligence.modelDescriptions.maximumQuality" },
] satisfies Array<{
  id: string;
  label: string;
  descKey: AnthropicModelDescriptionKey;
}>;

export function useApiKeyStatus() {
  const { data: maskedKey, isSuccess } = useQuery({
    queryKey: ["apiKey"],
    queryFn: getApiKey,
  });

  return {
    maskedKey,
    isConfigured: !!maskedKey,
    // The daemon has actually answered. `isConfigured` alone cannot tell
    // "no key" apart from "not asked yet", and callers that gate copy on the
    // answer must not treat the pending state as a no.
    isResolved: isSuccess,
  };
}

/** The native Anthropic key form: StatusChip, masked-key/clear or key-input
 *  branch, get-key link, error line, no-key guidance, and (once configured)
 *  the routine/synthesis model pickers. No <Card> wrapper and no <h3> title
 *  — every host (the Settings card, AnyProviderCard's Anthropic chip)
 *  supplies its own title, so this is just the body. */
export function AnthropicFields({
  showModelChoice = true,
  showNoKeyGuidance = true,
  settingsPresentation = false,
}: {
  showModelChoice?: boolean;
  showNoKeyGuidance?: boolean;
  settingsPresentation?: boolean;
}) {
  const { t } = useTranslation();
  const [keyInput, setKeyInput] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const queryClient = useQueryClient();
  const { maskedKey, isConfigured } = useApiKeyStatus();
  const helpDetails = settingsPresentation
    ? t("intelligence.settingsApiKeyHelp")
    : t("intelligence.apiKeyDescription");

  // A save/clear here changes what the daemon is actually serving (the
  // Anthropic key is hot-loaded even on 0.12), so the routing queries the
  // job summary rows read from must be invalidated alongside the key
  // itself — otherwise the rows keep showing stale state until remount.
  const invalidateIntelligenceQueries = () => {
    queryClient.invalidateQueries({ queryKey: ["apiKey"] });
    queryClient.invalidateQueries({ queryKey: ["external-llm"] });
    queryClient.invalidateQueries({ queryKey: ["external-llm-key-configured"] });
    queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] });
    queryClient.invalidateQueries({ queryKey: ACTIVITY_QUERY_KEY });
  };

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      await setApiKey(keyInput);
      setKeyInput("");
      // A key with no job pinned to it would change nothing: choose it for
      // any job that has no model yet.
      await fillUnsetPinsAfterSave();
      invalidateIntelligenceQueries();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleClear = async () => {
    setSaving(true);
    try {
      await setApiKey("");
      invalidateIntelligenceQueries();
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className={`flex flex-col ${settingsPresentation ? "settings-anthropic-fields" : ""}`} style={{ gap: "10px" }}>
      {!settingsPresentation && <>
        <div className="flex items-center justify-end">
          <StatusChip
            state={isConfigured ? { kind: "up" } : { kind: "idle" }}
            label={isConfigured ? t("intelligence.connected") : t("intelligence.notConfigured")}
          />
        </div>
        <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
          {t("intelligence.apiKeyDescription")}
        </p>
      </>}

      {settingsPresentation ? (
        <div className="settings-anthropic-key-section">
          <div className="settings-anthropic-key-row">
            <div className="settings-anthropic-key-label">
              <label htmlFor="anthropic-api-key">{t("externalProvider.apiKeyLabel")}</label>
              <SettingsHelp title={t("externalProvider.apiKeyLabel")} details={helpDetails} />
              {!isConfigured && (
                <button
                  type="button"
                  className="settings-intelligence-get-key-action"
                  onClick={() => shellOpen("https://console.anthropic.com/settings/keys")}
                >
                  {t("externalProvider.getKeyLink")}
                </button>
              )}
            </div>
            <div className="settings-anthropic-key-control">
              {isConfigured ? (
                <span className="settings-anthropic-key-value" aria-label={t("externalProvider.apiKeyLabel")}>{maskedKey}</span>
              ) : (
                <Input
                  id="anthropic-api-key"
                  type="password"
                  mono
                  value={keyInput}
                  onChange={(e) => setKeyInput(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && keyInput) handleSave(); }}
                  placeholder={t("intelligence.apiKeyPlaceholder")}
                />
              )}
              {isConfigured ? (
                <Button variant="ghost" size="sm" onClick={handleClear} disabled={saving}>
                  {t("intelligence.clear")}
                </Button>
              ) : (
                <Button variant="primary" size="sm" onClick={handleSave} disabled={saving || !keyInput} loading={saving}>
                  {t("intelligence.save")}
                </Button>
              )}
            </div>
          </div>
        </div>
      ) : isConfigured ? (
        <div className="flex items-center gap-2">
          <span
            className="flex-1 px-3 py-1.5 rounded-md"
            style={{
              fontFamily: "var(--mem-font-mono)",
              fontSize: "var(--mem-text-sm)",
              color: "var(--mem-text-secondary)",
              backgroundColor: "var(--mem-hover)",
            }}
          >
            {maskedKey}
          </span>
          <Button variant="ghost" size="sm" onClick={handleClear} disabled={saving}>
            {t("intelligence.clear")}
          </Button>
        </div>
      ) : (
        <div className="flex items-end gap-2">
          <div className="flex-1">
            <Field label={t("externalProvider.apiKeyLabel")} htmlFor="anthropic-api-key">
              <Input
                type="password"
                mono
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && keyInput) handleSave(); }}
                placeholder="sk-ant-api03-..."
              />
            </Field>
          </div>
          <Button variant="primary" size="sm" onClick={handleSave} disabled={saving || !keyInput} loading={saving}>
            {t("intelligence.save")}
          </Button>
        </div>
      )}

      {!settingsPresentation && !isConfigured && (
        <Button variant="ghost" size="sm" onClick={() => shellOpen("https://console.anthropic.com/settings/keys")} className="self-start">
          {t("externalProvider.getKeyLink")}
        </Button>
      )}

      {error && (
        <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)" }}>
          {error}
        </p>
      )}

      {!isConfigured && showNoKeyGuidance && (
        <div
          className="rounded-lg"
          style={{
            padding: "10px 14px",
            backgroundColor: "var(--mem-hover)",
            fontSize: "var(--mem-text-sm)",
            color: "var(--mem-text-secondary)",
            lineHeight: 1.6,
          }}
        >
          <div style={{ fontWeight: 500, color: "var(--mem-text)", marginBottom: 2, fontSize: "var(--mem-text-sm)" }}>
            {t("intelligence.synthesisCloudRecommendation")}
          </div>
          <div>{t("intelligence.memorySafe")}</div>
          <div style={{ marginTop: 6 }}>{t("intelligence.addApiKey")}</div>
        </div>
      )}

      {isConfigured && showModelChoice && <ModelChoiceSection />}
    </div>
  );
}

/** Routine-model half of the picker — the task row uses its compact form while
 *  ModelChoiceSection also renders the full form below the Anthropic key form. */
export function RoutineModelSelect({ compact = false }: { compact?: boolean } = {}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: modelChoice } = useQuery({
    queryKey: ["modelChoice"],
    queryFn: getModelChoice,
  });
  const [routineModel, synthesisModel] = modelChoice ?? [null, null];

  return (
    <div className="settings-task-field flex items-center justify-between">
      <div>
        <div style={{ fontSize: "var(--mem-text-base)", fontWeight: 500, color: "var(--mem-text)", fontFamily: "var(--mem-font-body)" }}>
          {compact ? t("intelligence.modelLabel") : t("intelligence.routineModel")}
        </div>
        {!compact && <div style={{ fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)", fontFamily: "var(--mem-font-body)" }}>{t("intelligence.routineModelDescription")}</div>}
      </div>
      <div className="settings-task-control shrink-0 w-fit">
        <Select
          size="sm"
          aria-label={t("intelligence.chooseRoutineModel")}
          value={routineModel ?? "claude-haiku-4-5-20251001"}
          onChange={async (e) => {
            await setModelChoice(e.target.value, synthesisModel);
            queryClient.invalidateQueries({ queryKey: ["modelChoice"] });
            queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] });
          }}
        >
          {ANTHROPIC_MODELS.map((m) => (
            <option key={m.id} value={m.id}>{compact ? m.label : `${m.label} - ${t(m.descKey)}`}</option>
          ))}
        </Select>
      </div>
    </div>
  );
}

/** Synthesis-model half — see RoutineModelSelect's doc comment. */
export function SynthesisModelSelect({ compact = false }: { compact?: boolean } = {}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: modelChoice } = useQuery({
    queryKey: ["modelChoice"],
    queryFn: getModelChoice,
  });
  const [routineModel, synthesisModel] = modelChoice ?? [null, null];

  return (
    <div className="settings-task-field flex items-center justify-between">
      <div>
        <div style={{ fontSize: "var(--mem-text-base)", fontWeight: 500, color: "var(--mem-text)", fontFamily: "var(--mem-font-body)" }}>
          {compact ? t("intelligence.modelLabel") : t("intelligence.synthesisModel")}
        </div>
        {!compact && <div style={{ fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)", fontFamily: "var(--mem-font-body)" }}>{t("intelligence.synthesisModelDescription")}</div>}
      </div>
      <div className="settings-task-control shrink-0 w-fit">
        <Select
          size="sm"
          aria-label={t("intelligence.chooseSynthesisModel")}
          value={synthesisModel ?? "claude-sonnet-4-6"}
          onChange={async (e) => {
            await setModelChoice(routineModel, e.target.value);
            queryClient.invalidateQueries({ queryKey: ["modelChoice"] });
            queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] });
          }}
        >
          {ANTHROPIC_MODELS.map((m) => (
            <option key={m.id} value={m.id}>{compact ? m.label : `${m.label} - ${t(m.descKey)}`}</option>
          ))}
        </Select>
      </div>
    </div>
  );
}

export function ModelChoiceSection() {
  return (
    <div className="mt-4 pt-3" style={{ borderTop: "1px solid var(--mem-border)" }}>
      <div className="mb-3">
        <RoutineModelSelect />
      </div>
      <SynthesisModelSelect />
    </div>
  );
}

export function OnDeviceModelCard({
  deferDownload = false,
  onModelChosen,
  bare = false,
}: {
  /** Wizard step 2: record the choice, don't download here — step 5 does
   *  the download and proves it landed (`loaded === modelId`), not just
   *  that the request was sent. Settings (no props) keeps the immediate
   *  download this card has always done. */
  deferDownload?: boolean;
  onModelChosen?: (modelId: string | null) => void;
  /** Skip the <Card> wrapper and the title/status-chip header — the host
   *  (a disclosure row) already shows both. Default false keeps every
   *  existing call site unchanged. */
  bare?: boolean;
} = {}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [pickedId, setPickedId] = useState<string | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { data: modelData } = useQuery({
    queryKey: ["onDeviceModel"],
    queryFn: getOnDeviceModel,
  });
  const { data: systemInfo } = useQuery({
    queryKey: ["systemInfo"],
    queryFn: getSystemInfo,
  });

  const models = modelData?.models ?? [];
  const loadedId = modelData?.loaded ?? null;
  const selectedId = modelData?.selected ?? null;
  const currentId = pickedId ?? loadedId ?? selectedId ?? models[0]?.id ?? null;
  const current = currentId ? models.find((m) => m.id === currentId) : null;

  const isLoaded = !!current && loadedId === current.id;
  const needsDownload = !!current && !current.cached;
  const canLoad = !!current && current.cached && !isLoaded;
  const ramOk = systemInfo ? systemInfo.total_ram_gb + 0.5 >= (current?.ram_required_gb ?? 0) : true;

  // Report the current pick upward whenever it changes — mount (once the
  // catalog loads) and every dropdown change. A ref keeps this from
  // re-firing on every parent render just because it passed a fresh
  // closure.
  const onModelChosenRef = useRef(onModelChosen);
  useEffect(() => {
    onModelChosenRef.current = onModelChosen;
  }, [onModelChosen]);
  useEffect(() => {
    if (deferDownload) onModelChosenRef.current?.(currentId);
  }, [deferDownload, currentId]);

  const handleDownload = async () => {
    if (!current) return;
    setDownloading(true);
    setError(null);
    try {
      await downloadOnDeviceModel(current.id);
      // Only Settings reaches this (the wizard defers the download to its
      // Setting up step, which fills pins when that download resolves), so a
      // model turned on here is chosen for any job without one.
      await fillUnsetPinsAfterSave();
      queryClient.invalidateQueries({ queryKey: ["onDeviceModel"] });
      queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] });
      queryClient.invalidateQueries({ queryKey: ACTIVITY_QUERY_KEY });
      setPickedId(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setDownloading(false);
    }
  };

  const statusBadge = isLoaded ? t("intelligence.running") : t("intelligence.notLoaded");

  const body = (
      <div className="flex flex-col" style={{ gap: "10px" }}>
        {!bare && (
          <div className="flex items-center justify-between">
            <h3 style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-lg)", fontWeight: 600, color: "var(--mem-text)" }}>
              {t("intelligence.onDeviceModel")}
            </h3>
            <StatusChip state={isLoaded ? { kind: "up" } : { kind: "idle" }} label={statusBadge} />
          </div>
        )}
        <p
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-text-tertiary)",
          }}
        >
          {t("intelligence.localModelDescription")}
          {systemInfo && (
            <>
              {" "}
              {t("intelligence.systemInfo", {
                ram: systemInfo.total_ram_gb.toFixed(0),
                accelerators: [
                  systemInfo.has_metal ? " · Metal GPU" : "",
                  systemInfo.has_cuda ? " · CUDA GPU" : "",
                ].join(""),
              })}
            </>
          )}
        </p>

        <div className="flex items-start gap-2">
          {current && (
            <span
              data-testid="on-device-model-spec"
              className="flex-1 min-w-0"
              style={{
                fontFamily: "var(--mem-font-mono)",
                fontSize: "var(--mem-text-xs)",
                color: "var(--mem-text-secondary)",
              }}
            >
              {t("intelligence.modelDetails", {
                params: current.param_count,
                downloadSize: current.file_size_gb.toFixed(1),
                ram: current.ram_required_gb.toFixed(0),
              })}
              {current.cached && !isLoaded && t("intelligence.downloadedNotLoaded")}
              {!current.cached && t("intelligence.notDownloaded")}
            </span>
          )}

          {models.length > 0 ? (
            <div className="shrink-0 w-fit">
              <Select
                size="sm"
                mono
                aria-label={t("intelligence.chooseOnDeviceModel")}
                value={currentId ?? ""}
                onChange={(e) => setPickedId(e.target.value)}
              >
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.display_name}
                  </option>
                ))}
              </Select>
            </div>
          ) : (
            <span
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-xs)",
                color: "var(--mem-text-tertiary)",
              }}
            >
              {t("intelligence.modelCatalogUnavailable")}
            </span>
          )}

          {needsDownload && !deferDownload && (
            <Button
              variant="primary"
              size="sm"
              onClick={handleDownload}
              disabled={downloading || !ramOk}
              loading={downloading}
              className="whitespace-nowrap shrink-0"
              title={!ramOk ? t("intelligence.notEnoughRamTitle") : undefined}
            >
              {t("intelligence.downloadSize", { size: current?.file_size_gb.toFixed(1) })}
            </Button>
          )}
          {canLoad && !deferDownload && (
            <Button
              variant="primary"
              size="sm"
              onClick={handleDownload}
              disabled={downloading}
              loading={downloading}
              className="whitespace-nowrap shrink-0"
            >
              {t("intelligence.load")}
            </Button>
          )}
        </div>

        {deferDownload && !isLoaded && current && (
          <p
            data-testid="on-device-model-deferred-note"
            style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}
          >
            {t("intelligence.willDownloadOnSetup")}
          </p>
        )}
        {downloading && !deferDownload && (
          <p
            style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "var(--mem-text-xs)",
              color: "var(--mem-text-tertiary)",
            }}
          >
            {t("intelligence.downloadMayTake", { size: current?.file_size_gb.toFixed(1) })}
          </p>
        )}
        {error && (
          <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)" }}>
            {error}
          </p>
        )}
        {!ramOk && current && !downloading && (
          <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-xs)", color: "var(--mem-status-warning-text)" }}>
            {t("intelligence.ramWarning", {
              model: current.display_name,
              required: current.ram_required_gb.toFixed(0),
              available: systemInfo?.total_ram_gb.toFixed(0),
            })}
          </p>
        )}
      </div>
  );

  return bare ? body : <Card padding="card">{body}</Card>;
}

/** Settings-only manager for the installed on-device model catalog. Several
 *  models may remain downloaded, while the daemon has one active model slot.
 *  The wizard keeps using OnDeviceModelCard and its existing single-pick flow. */
/** Which models the on-device list shows: the computer's shortlist (plus the
 * model in use, wherever it sits), or every model once "Show all" is open or
 * when the daemon gave no shortlist (an older daemon, or memory unreadable). */
export function visibleDeviceModels(models: OnDeviceModelEntry[], loadedId: string | null, showAll: boolean): OnDeviceModelEntry[] {
  const hasShortlist = models.some((model) => model.shortlisted);
  if (showAll || !hasShortlist) return models;
  return models.filter((model) => model.shortlisted || model.id === loadedId);
}

const deviceTextStyle = { fontFamily: "var(--mem-font-body)" } as const;

export function OnDeviceModelManager() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [activatingId, setActivatingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const modelQuery = useQuery({
    queryKey: ["onDeviceModel"],
    queryFn: getOnDeviceModel,
  });
  const models = modelQuery.data?.models ?? [];
  const loadedId = modelQuery.data?.loaded ?? null;
  const totalRam = modelQuery.data?.total_ram_gb ?? null;

  const activate = async (modelId: string) => {
    const model = models.find((item) => item.id === modelId);
    if (!model || activatingId) return;
    setActivatingId(modelId);
    setError(null);
    try {
      // The daemon operation downloads uncached models and activates cached
      // ones. It retains cached files when switching the single active slot.
      await downloadOnDeviceModel(modelId);
      await fillUnsetPinsAfterSave();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["onDeviceModel"] }),
        queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] }),
        queryClient.invalidateQueries({ queryKey: ACTIVITY_QUERY_KEY }),
      ]);
    } catch (e) {
      setError(String(e));
    } finally {
      setActivatingId(null);
    }
  };

  if (models.length === 0) {
    return (
      <p style={{ ...deviceTextStyle, fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)" }}>
        {modelQuery.isPending ? t("intelligence.loading") : t("intelligence.modelCatalogUnavailable")}
      </p>
    );
  }

  const visible = visibleDeviceModels(models, loadedId, showAll);
  const canToggle = models.some((model) => model.shortlisted) && models.length > visibleDeviceModels(models, loadedId, false).length;

  return (
    <div className="settings-device-models flex flex-col gap-1">
      {totalRam !== null && (
        <p className="pb-2" style={{ ...deviceTextStyle, fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)" }}>
          {t("intelligence.thisComputerRam", { ram: Math.round(totalRam) })}
        </p>
      )}
      {visible.map((model) => {
        const inUse = loadedId === model.id;
        const activating = activatingId === model.id;
        const fits = model.fits !== false;
        return (
          <div key={model.id} className="settings-device-model flex items-center justify-between gap-4 py-2.5" data-model={model.id} data-fits={fits}>
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-baseline gap-x-2">
                <span style={{ ...deviceTextStyle, fontSize: "var(--mem-text-md)", fontWeight: 500, color: fits ? "var(--mem-text)" : "var(--mem-text-tertiary)" }}>
                  {model.display_name}
                </span>
                {model.recommended && (
                  <span style={{ ...deviceTextStyle, fontSize: "var(--mem-text-xs)", fontWeight: 600, color: "var(--mem-accent-indigo)" }}>
                    {t("intelligence.recommendedModel")}
                  </span>
                )}
              </div>
              <p style={{ ...deviceTextStyle, fontSize: "var(--mem-text-xs)", color: "var(--mem-text-tertiary)" }}>
                {activating && !model.cached
                  ? t("intelligence.downloadMayTake", { size: model.file_size_gb.toFixed(1) })
                  : fits
                    ? t("intelligence.modelSize", { size: model.file_size_gb.toFixed(1) })
                    : t("intelligence.modelNeedsRam", { ram: model.ram_required_gb.toFixed(0) })}
              </p>
            </div>
            {inUse ? (
              <span className="shrink-0" style={{ ...deviceTextStyle, fontSize: "var(--mem-text-sm)", fontWeight: 500, color: "var(--mem-accent-sage-text)" }}>
                {t("intelligence.modelInUse")}
              </span>
            ) : fits ? (
              <Button
                variant={model.recommended ? "primary" : "ghost"}
                size="sm"
                disabled={activatingId !== null}
                loading={activating}
                onClick={() => activate(model.id)}
                className="shrink-0 whitespace-nowrap"
              >
                {model.cached ? t("intelligence.modelUse") : t("intelligence.modelDownload")}
              </Button>
            ) : null}
          </div>
        );
      })}
      {canToggle && (
        <div className="-ml-3 pt-1">
          <Button variant="ghost" size="sm" onClick={() => setShowAll((open) => !open)} aria-expanded={showAll}>
            {showAll ? t("intelligence.showFewerModels") : t("intelligence.showAllModels", { count: models.length })}
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="pt-2" style={{ ...deviceTextStyle, fontSize: "var(--mem-text-xs)", color: "var(--mem-status-danger-text)" }}>
          {error}
        </p>
      )}
    </div>
  );
}
