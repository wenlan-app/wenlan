// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import {
  getProfile,
  updateProfile,
  setAvatar,
  removeAvatar,
  setSetupCompleted,
  isRunAtLoginEnabled,
  setRunAtLogin,
  getTelemetryStatus,
  setTelemetryEnabled,
  exportPagesAsOkf,
  getKnowledgePath,
  openFile,
  daemonErrorMessage,
  isOkfExportDaemonTooOld,
} from "../../../../lib/tauri";
import { type Theme, useTheme } from "../../../../lib/theme";
import {
  readStoredLocalePreference,
  setLocalePreference,
  type StoredLocale,
} from "../../../../i18n";
import {
  Button,
  ConfirmActionButton,
  Input,
  SegmentedControl,
  Select,
  SettingRow,
} from "../primitives";
import { SettingsGroup } from "../SettingsView";
import ProfileAvatar from "../../ProfileAvatar";
import { replayPageTour, usePageTourState } from "../../../onboarding/PageTour";
import { resetTabIntros } from "../../../onboarding/TabIntro";

type ThemeLabelKey =
  | "settings.theme.auto"
  | "settings.theme.light"
  | "settings.theme.dark";

const THEME_OPTIONS: { value: Theme; labelKey: ThemeLabelKey; icon: React.ReactNode }[] = [
  {
    value: "system",
    labelKey: "settings.theme.auto",
    icon: (
      <svg aria-hidden="true" className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
      </svg>
    ),
  },
  {
    value: "light",
    labelKey: "settings.theme.light",
    icon: (
      <svg aria-hidden="true" className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
      </svg>
    ),
  },
  {
    value: "dark",
    labelKey: "settings.theme.dark",
    icon: (
      <svg aria-hidden="true" className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
      </svg>
    ),
  },
];

interface ProfileUpdateFields {
  name?: string;
  displayName?: string;
  bio?: string;
}

function ProfileSettingsBlock() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: profile } = useQuery({
    queryKey: ["profile"],
    queryFn: getProfile,
  });
  const [nameDraft, setNameDraft] = useState("");

  const displayName = profile?.display_name || profile?.name || "";

  useEffect(() => {
    setNameDraft(displayName);
  }, [displayName]);

  const profileMutation = useMutation({
    mutationFn: (fields: ProfileUpdateFields) => {
      if (!profile) return Promise.resolve();
      return updateProfile(profile.id, fields.name, fields.displayName, undefined, fields.bio);
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["profile"] }),
  });

  const avatarMutation = useMutation({
    mutationFn: (path: string) => setAvatar(path),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["profile"] }),
  });

  const removeAvatarMutation = useMutation({
    mutationFn: removeAvatar,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["profile"] }),
  });

  const saveName = () => {
    if (!profile) return;
    const next = nameDraft.trim();
    const current = displayName.trim();
    if (!next || next === current) {
      setNameDraft(displayName);
      return;
    }
    profileMutation.mutate({ name: next, displayName: next });
  };

  const handlePickAvatar = async () => {
    const selected = await open({
      title: t("settings.profile.choosePhoto"),
      filters: [{ name: t("settings.profile.images"), extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
    });
    if (typeof selected === "string") {
      avatarMutation.mutate(selected);
    }
  };

  if (!profile) return null;

  return (
    <SettingsGroup heading={t("settings.profile.label")} className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <div className="settings-profile-row px-5 py-4">
          <div className="settings-profile-row-content flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-md)", fontWeight: 500, color: "var(--mem-text)" }}>
                {t("settings.profile.photo")}
              </div>
            </div>
            <div className="settings-profile-controls flex items-center gap-2">
              <ProfileAvatar
                avatarPath={profile.avatar_path}
                displayName={displayName}
                size={32}
                fontSize={13}
              />
              <Button variant="secondary" size="sm" onClick={handlePickAvatar}>
                {t("settings.profile.changePhoto")}
              </Button>
              {profile.avatar_path && (
                <Button variant="ghost" size="sm" onClick={() => removeAvatarMutation.mutate()}>
                  {t("settings.profile.removePhoto")}
                </Button>
              )}
            </div>
          </div>
        </div>
        <div className="settings-profile-row px-5 py-4">
          <div className="settings-profile-row-content flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-md)", fontWeight: 500, color: "var(--mem-text)" }}>
                {t("settings.profile.displayName")}
              </div>
            </div>
            <div>
              <Input
                aria-label={t("settings.profile.displayName")}
                value={nameDraft}
                onChange={(event) => setNameDraft(event.target.value)}
                onBlur={saveName}
                onKeyDown={(event) => {
                  if (event.key === "Enter") event.currentTarget.blur();
                  if (event.key === "Escape") {
                    setNameDraft(displayName);
                    event.currentTarget.blur();
                  }
                }}
                size={Math.min(24, Math.max(6, nameDraft.length))}
                className="settings-profile-input settings-profile-input-inline"
              />
            </div>
          </div>
        </div>
    </SettingsGroup>
  );
}

/**
 * The sentence a rejected Tauri command carries, or `null` when there is no
 * error to show.
 *
 * A command declared `Result<_, String>` rejects with the bare string, so the
 * common case is not an `Error` at all. Anything else that is not a non-empty
 * string is deliberately dropped rather than rendered as `[object Object]` or
 * an empty red line: a row that shows an unreadable error is worse than a row
 * that shows none, because it claims to be telling the user something.
 */
function errorMessage(error: unknown): string | null {
  if (typeof error === "string") return error.trim() || null;
  if (error instanceof Error) return error.message.trim() || null;
  return null;
}

export default function GeneralSection() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [theme, setThemeValue] = useTheme();
  const [languagePreference, setLanguagePreference] = useState<StoredLocale>(
    () => readStoredLocalePreference(),
  );

  // ── Run at login ───────────────────────────────────────────────────
  const runAtLoginQuery = useQuery({
    queryKey: ["runAtLogin"],
    queryFn: isRunAtLoginEnabled,
  });
  // `is_run_at_login_enabled` errors when launchctl could not be read, rather
  // than answering `false`. Rendering that as an off toggle would tell the
  // user the feature is disabled while launchd may still start Wenlan every
  // boot, so the row says it could not be read instead.
  const runAtLoginUnreadable = runAtLoginQuery.isError;
  const runAtLoginMutation = useMutation({
    mutationFn: setRunAtLogin,
    onSuccess: () => runAtLoginQuery.refetch(),
  });
  // `set_run_at_login` refuses the handover when it could not confirm that
  // Wenlan's own daemon stopped: registering launchd against a port the old
  // daemon still holds makes two owners. The refusal names which of the two
  // happened and what to do about it, and that sentence only exists in the
  // rejected `invoke` promise -- without this the toggle just fails to move
  // and the user is told nothing. `useMutation` clears `error` on the next
  // `mutate`, so a successful retry removes the line.
  const runAtLoginRefusal = errorMessage(runAtLoginMutation.error);
  // The value THIS read produced, or `undefined` when the current read did not
  // succeed.
  //
  // Round 5, D2. This used to be `runAtLoginQuery.data`, and `data ===
  // undefined` answers a strictly narrower question than the one the row needs:
  // it distinguishes "no value has ever been cached" from "a boolean is
  // cached", NOT "the current read succeeded" from "the current read failed".
  // React Query RETAINS the last `data` across a failed refetch, so the
  // reachable state `data === false, isError === true` painted an enabled,
  // left-positioned "off" switch — a claim about launchd from an earlier
  // instant — beside the very notice saying the state could not be read. A
  // screen reader heard "not pressed", and a click computed `mutate(true)` from
  // that stale reading (symmetrically, a retained `true` sent `mutate(false)`).
  // `isSuccess` is the status of the LATEST read, so a failed refresh drops the
  // row back to unknown instead of quietly re-asserting the old answer.
  const runAtLoginState = runAtLoginQuery.isSuccess ? runAtLoginQuery.data : undefined;
  // Both failures can be live at once -- the state went unreadable AND a
  // handover was refused -- and they say different things. Joined rather than
  // ranked, because the refusal is the half that names the remedy.
  const runAtLoginProblem =
    [
      runAtLoginUnreadable ? t("settings.general.runAtLoginUnreadable") : null,
      runAtLoginRefusal,
    ]
      .filter(Boolean)
      .join(" ") || null;

  // Product telemetry is daemon-owned and opt-in. Until the latest read has
  // succeeded with an available daemon, the switch must not paint an off
  // state or derive a write from a value nobody measured.
  const telemetryQuery = useQuery({
    queryKey: ["telemetryStatus"],
    queryFn: getTelemetryStatus,
  });
  const telemetryStatus = telemetryQuery.isSuccess ? telemetryQuery.data : undefined;
  const telemetryAvailable = telemetryStatus?.available === true;
  const telemetryUnreadable = telemetryQuery.isError;
  const telemetryUnavailable = telemetryStatus?.available === false;
  const telemetryMutation = useMutation({
    mutationFn: setTelemetryEnabled,
    onSettled: async () => {
      // The PUT response is useful for the immediate path, but a fresh GET is
      // the authoritative reload. This also makes a failed save visible as
      // the daemon's actual state rather than a client-side optimistic guess.
      await telemetryQuery.refetch();
    },
  });
  const telemetryProblem = telemetryUnreadable
    ? t("settings.general.telemetryReadFailed")
    : telemetryUnavailable
      ? t("settings.general.telemetryUnavailable")
      : null;
  const telemetrySaveError = telemetryMutation.isError
    ? t("settings.general.telemetrySaveFailed")
    : null;
  const telemetryUnknown =
    telemetryStatus === undefined || (!telemetryAvailable && !telemetryStatus.enabled) || telemetryMutation.isPending;
  const telemetryPending =
    telemetryAvailable && telemetryStatus.enabled && telemetryStatus.pending_operations > 0
      ? t("settings.general.telemetryPending", {
          count: telemetryStatus.pending_operations,
        })
      : null;

  return (
    <>
      <ProfileSettingsBlock />
      <SettingsGroup heading={t("settings.general.appSection")} className="mem-fade-up" style={{ animationDelay: "0ms" }}>
          {/* Theme — folded into General; previously its own "Appearance" sidebar entry. */}
          <SettingRow
            title={t("settings.theme.label")}
            description=""
            control={
              <SegmentedControl
                aria-label={t("settings.theme.label")}
                options={THEME_OPTIONS.map((opt) => ({
                  value: opt.value,
                  label: t(opt.labelKey),
                  icon: opt.icon,
                }))}
                value={theme}
                onChange={setThemeValue}
              />
            }
          />
          <SettingRow
            title={t("settings.language.label")}
            description=""
            control={
              <div className="w-fit shrink-0">
                <Select
                  size="sm"
                  aria-label={t("settings.language.label")}
                  value={languagePreference}
                  onChange={(event) => {
                    const nextPreference = event.currentTarget.value as StoredLocale;
                    setLanguagePreference(nextPreference);
                    void setLocalePreference(nextPreference);
                  }}
                >
                  <option value="system">{t("settings.language.system")}</option>
                  <option value="en">{t("settings.language.english")}</option>
                  <option value="zh-Hans">{t("settings.language.simplifiedChinese")}</option>
                  <option value="zh-Hant">{t("settings.language.traditionalChinese")}</option>
                </Select>
              </div>
            }
          />
          <SettingRow
            title={t("settings.general.runAtLoginTitle")}
            description=""
            details={t("settings.general.runAtLoginDescription")}
            enabled={runAtLoginState ?? false}
            // Not measured is not `false`, and a value left over from a read
            // that has since FAILED is not measured either. Without this the
            // row paints the switch off -- a claim about launchd nobody read,
            // or read at some earlier instant -- and the click below computes
            // its new value from that same fiction.
            valueUnknown={runAtLoginState === undefined}
            unknownState={runAtLoginUnreadable ? "unavailable" : "loading"}
            onToggle={() => {
              // `!(undefined ?? false)` is `true` whatever launchd actually
              // holds, so an unread state must not reach `mutate` even if the
              // disabled control were somehow activated. The measurement, not
              // the widget, is what makes this action safe.
              if (runAtLoginState === undefined) return;
              runAtLoginMutation.mutate(!runAtLoginState);
            }}
            // Both sentences, when there are both. These are independent
            // failures -- the state became unreadable AND one attempt to
            // change it was refused -- and showing only the first drops the
            // one that names what to actually do about it.
            error={runAtLoginProblem}
          />
          <SettingRow
            title={t("settings.general.telemetryTitle")}
            description=""
            details={t("settings.general.telemetryDescription")}
            detailsSummary={t("settings.general.telemetrySummary")}
            enabled={telemetryStatus?.enabled ?? false}
            valueUnknown={telemetryUnknown}
            unknownState={telemetryMutation.isPending
              ? "saving"
              : telemetryQuery.isPending
                ? "loading"
                : "unavailable"}
            onToggle={() => {
              if (telemetryMutation.isPending || !telemetryStatus) return;
              const nextEnabled = !telemetryStatus.enabled;
              // A kill switch or debug build blocks enabling collection, not
              // revoking persisted consent before the next normal launch.
              if (nextEnabled && !telemetryAvailable) return;
              telemetryMutation.mutate(nextEnabled);
            }}
            statusLine={telemetryPending}
            warning={telemetryProblem}
            error={telemetrySaveError}
          />
          {/* Re-run setup wizard — a proper row with an inline two-step
              confirm; data is preserved regardless. */}
          <SettingRow
            title={t("settings.general.rerunSetup")}
            description=""
            details={t("settings.general.rerunSetupConfirm")}
            control={
              <ConfirmActionButton
                variant="secondary"
                size="sm"
                confirmLabel={t("settings.agents.confirm")}
                cancelLabel={t("settings.agents.cancel")}
                onConfirm={async () => {
                  await setSetupCompleted(false);
                  queryClient.invalidateQueries({ queryKey: ["shouldShowWizard"] });
                }}
              >
                {t("settings.general.rerunSetupGo")}
              </ConfirmActionButton>
            }
          />
          <PageTipsRow />
      </SettingsGroup>
      <LocalDataSettingsBlock />
      <ExportSettingsBlock />
    </>
  );
}

/** Brings the first-page tips back for the next page Wenlan writes. */
function PageTipsRow() {
  const { t } = useTranslation();
  const tour = usePageTourState();
  return (
    <SettingRow
      title={t("settings.general.pageTips")}
      description=""
      details={t("settings.general.pageTipsDetails")}
      statusLine={tour === "replay" ? t("settings.general.pageTipsQueued") : undefined}
      control={
        <Button
          variant="secondary"
          size="sm"
          disabled={tour === "replay"}
          onClick={() => {
            replayPageTour();
            resetTabIntros();
          }}
          data-testid="settings-page-tips-replay"
        >
          {t("settings.general.pageTipsGo")}
        </Button>
      }
    />
  );
}

function LocalDataSettingsBlock() {
  const { t } = useTranslation();
  const knowledgePathQuery = useQuery({
    queryKey: ["knowledgePath"],
    queryFn: getKnowledgePath,
  });
  const openFolderMutation = useMutation({ mutationFn: (path: string) => openFile(path) });
  const knowledgePath = typeof knowledgePathQuery.data === "string" && knowledgePathQuery.data.trim()
    ? knowledgePathQuery.data
    : undefined;
  const locationUnavailable = knowledgePathQuery.isError || (knowledgePathQuery.isSuccess && !knowledgePath);
  const openError = openFolderMutation.isError
    ? errorMessage(openFolderMutation.error) ?? t("settings.general.pageStorageOpenError")
    : null;
  const locationError = locationUnavailable
    ? t("settings.general.pageStorageUnavailable")
    : null;

  return (
    <SettingsGroup heading={t("settings.general.localData")} className="mem-fade-up" style={{ animationDelay: "0ms" }}>
      <SettingRow
        title={t("settings.general.pageStorageTitle")}
        description={t("settings.general.pageStorageDescription")}
        details={t("settings.general.pageStorageHelp")}
        error={locationError ?? openError}
        control={(
          <div className="settings-page-local-data-control">
            {knowledgePath ? (
              <>
                <span className="settings-page-local-data-path">{knowledgePath}</span>
                <Button
                  variant="secondary"
                  size="sm"
                  loading={openFolderMutation.isPending}
                  aria-busy={openFolderMutation.isPending}
                  onClick={() => openFolderMutation.mutate(knowledgePath)}
                >
                  {t("settings.general.openFolder")}
                </Button>
              </>
            ) : knowledgePathQuery.isPending ? (
              <span role="status" aria-label={t("settings.general.pageStorageLoading")}>
                {t("settings.general.pageStorageLoading")}
              </span>
            ) : null}
          </div>
        )}
      />
    </SettingsGroup>
  );
}

/**
 * Export every page, across all Spaces, as an OKF v0.2 bundle.
 *
 * The daemon owns the bundle and the rules for which folders are safe to
 * write, so this block only picks a folder and reports what came back. A
 * refused folder (not empty and not an earlier Wenlan OKF export) arrives as a
 * 409 whose `error` sentence is the useful part; the HTTP prefix is not.
 */
function ExportSettingsBlock() {
  const { t } = useTranslation();
  const exportMutation = useMutation({
    mutationFn: async (targetDir: string) => ({
      targetDir,
      stats: await exportPagesAsOkf(targetDir),
    }),
  });

  const handleChooseFolder = async () => {
    const selected = await open({
      title: t("settings.general.exportOkfDialogTitle"),
      directory: true,
      multiple: false,
    });
    // `null` is a cancelled picker: nothing to export, and no reason to clear
    // the last result the row is showing.
    if (typeof selected !== "string") return;
    exportMutation.mutate(selected);
  };

  const result = exportMutation.isSuccess ? exportMutation.data : undefined;
  const exportReason = !exportMutation.isError
    ? null
    : isOkfExportDaemonTooOld(exportMutation.error)
      ? t("settings.general.exportOkfDaemonTooOld")
      : daemonErrorMessage(exportMutation.error) ?? errorMessage(exportMutation.error);
  const exportError = exportMutation.isError
    ? exportReason
      ? t("settings.general.exportOkfError", { reason: exportReason })
      : t("settings.general.exportOkfErrorUnknown")
    : null;
  const statusLines = exportMutation.isPending
    ? [t("settings.general.exportOkfRunning")]
    : result
      ? [
          t("settings.general.exportOkfDone", {
            count: result.stats.exported,
            path: result.targetDir,
          }),
          result.stats.skipped > 0
            ? t("settings.general.exportOkfSkipped", { count: result.stats.skipped })
            : null,
        ].filter((line): line is string => line !== null)
      : [];

  return (
    <SettingsGroup heading={t("settings.general.exportSection")} className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SettingRow
          title={t("settings.general.exportOkfTitle")}
          description=""
          details={t("settings.general.exportOkfDescription")}
          control={
            <Button
              variant="secondary"
              size="sm"
              className="whitespace-nowrap"
              loading={exportMutation.isPending}
              aria-busy={exportMutation.isPending}
              onClick={handleChooseFolder}
            >
              {t("settings.general.exportOkfChoose")}
            </Button>
          }
          statusLine={
            // Always mounted so a screen reader hears the result arrive, not
            // only the next time the row re-renders.
            <div role="status" aria-live="polite">
              {statusLines.map((line) => (
                <p
                  key={line}
                  style={{
                    fontFamily: "var(--mem-font-body)",
                    fontSize: "var(--mem-text-description)",
                    color: "var(--mem-text-secondary)",
                    lineHeight: "1.5",
                    overflowWrap: "anywhere",
                  }}
                >
                  {line}
                </p>
              ))}
            </div>
          }
          warning={
            result && result.stats.failed > 0
              ? t("settings.general.exportOkfFailed", { count: result.stats.failed })
              : null
          }
          error={exportError}
        />
    </SettingsGroup>
  );
}
