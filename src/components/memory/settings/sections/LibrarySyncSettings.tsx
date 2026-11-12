// SPDX-License-Identifier: AGPL-3.0-only
// Settings › General › Other computers: keep the whole library the same on
// this person's other computers through a cloud folder.
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  lookInSharedFolder,
  revealSharedFolder,
  setLibraryMainDevice,
  shareLibrary,
  stopSharingLibrary,
  type SharedFolder,
  type SharedStatus,
} from "../../../../lib/tauri";
import { ConfirmDialog } from "../../../ConfirmDialog";
import { relativeMs } from "../../page/format";
import { FolderIcon } from "../../sharing/ShareDialog";
import { SHARES_KEY, cloudName, shortFolder, useShares } from "../../sharing/sharing";
import { SettingsGroup } from "../SettingsView";
import { Button, Select, SettingRow } from "../primitives";
import "../../sharing/sharing.css";
import "./librarySync.css";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function libraryShare(shares: readonly SharedStatus[] | undefined): SharedStatus | undefined {
  return shares?.find((share) => share.kind === "library");
}

/** The line under the row's title once syncing is on. */
function syncLine(share: SharedStatus, t: TFunction, locale: string): string {
  const last = share.last;
  if (!last) return t("sharing.syncing");
  if (last.paused) return t("sharing.paused");
  if (last.counts.skipped > 0) return t("sharing.waiting", { count: last.counts.skipped });
  const cloud = cloudName(share.folder, t) ?? shortFolder(share.folder, t);
  const when = relativeMs(Date.parse(last.at), locale);
  return `${cloud} · ${t("librarySync.synced", { when })}`;
}

export default function LibrarySyncSettings() {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const queryClient = useQueryClient();
  const shares = useShares();
  const share = libraryShare(shares.data);
  const [dialog, setDialog] = useState(false);
  const [confirmingStop, setConfirmingStop] = useState(false);
  const [pickingMain, setPickingMain] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => queryClient.invalidateQueries({ queryKey: SHARES_KEY });
  const stop = async (confirmed: boolean) => {
    setConfirmingStop(false);
    if (!confirmed) return;
    try {
      await stopSharingLibrary();
      await refresh();
    } catch (caught) {
      setError(errorText(caught));
    }
  };
  const chooseMain = async (deviceId: string) => {
    setPickingMain(false);
    if (!deviceId || deviceId === share?.main_device) return;
    try {
      await setLibraryMainDevice(deviceId);
      await refresh();
    } catch (caught) {
      setError(errorText(caught));
    }
  };

  const last = share?.last;
  const problem = error ?? last?.error ?? null;
  const warning = last?.held_back ? t("librarySync.heldBack") : null;
  return (
    <SettingsGroup className="mem-fade-up" style={{ animationDelay: "0ms" }}>
      <SettingRow
        title={t("librarySync.title")}
        description={share ? syncLine(share, t, locale) : t("librarySync.hint")}
        enabled={Boolean(share)}
        valueUnknown={shares.data === undefined}
        unknownState={shares.isPending ? "loading" : "unavailable"}
        onToggle={() => {
          if (share) setConfirmingStop(true);
          else setDialog(true);
        }}
        warning={warning}
        error={problem ? t("librarySync.failed", { message: problem }) : null}
      />
      {share && (
        <ComputersRow
          share={share}
          t={t}
          pickingMain={pickingMain}
          onPickMain={() => setPickingMain(true)}
          onChooseMain={(id) => void chooseMain(id)}
        />
      )}
      {share && (
        <SettingRow
          title={t("librarySync.folder")}
          description={shortFolder(share.folder, t)}
          controlLayout="inline"
          control={(
            <div className="library-sync-buttons">
              <Button variant="secondary" size="sm" onClick={() => void revealSharedFolder(share.folder).catch(() => undefined)}>
                {t("librarySync.open")}
              </Button>
              <Button variant="secondary" size="sm" onClick={() => setDialog(true)}>
                {t("librarySync.change")}
              </Button>
            </div>
          )}
        />
      )}
      {dialog && <LibrarySyncDialog onClose={() => setDialog(false)} />}
      {confirmingStop && (
        <ConfirmDialog
          message={t("librarySync.stopConfirm")}
          confirmLabel={t("librarySync.stop")}
          cancelLabel={t("sharing.cancel")}
          onAnswer={(confirmed) => void stop(confirmed)}
        />
      )}
    </SettingsGroup>
  );
}

/** One row for the computers: which ones sync, which is this one, and the
 *  main one, with the way to choose another main computer. */
function ComputersRow({ share, t, pickingMain, onPickMain, onChooseMain }: {
  readonly share: SharedStatus;
  readonly t: TFunction;
  readonly pickingMain: boolean;
  readonly onPickMain: () => void;
  readonly onChooseMain: (deviceId: string) => void;
}) {
  const devices = share.devices ?? [];
  if (devices.length === 0) return null;
  const main = devices.find((device) => device.id === share.main_device);
  const label = (device: (typeof devices)[number]) => {
    const here = device.id === share.this_device;
    const isMain = device.id === main?.id;
    if (here && isMain) return t("librarySync.thisMain", { name: device.name });
    if (here) return t("librarySync.thisOne", { name: device.name });
    if (isMain) return t("librarySync.mainMark", { name: device.name });
    return device.name;
  };
  const control = main && devices.length > 1 ? (
    pickingMain ? (
      <Select
        size="sm"
        aria-label={t("librarySync.chooseMain")}
        value={main.id}
        onChange={(event) => onChooseMain(event.currentTarget.value)}
      >
        {devices.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
      </Select>
    ) : (
      <Button variant="secondary" size="sm" className="whitespace-nowrap" onClick={onPickMain}>
        {t("librarySync.changeMain")}
      </Button>
    )
  ) : null;
  return (
    <SettingRow
      title={t("librarySync.computers")}
      description={devices.map(label).join(t("librarySync.listJoin"))}
      details={main ? t("librarySync.mainHint") : undefined}
      controlLayout="inline"
      control={control}
    />
  );
}

/** Turn syncing on (or move it to another folder): choose the folder, then
 *  start, or merge when the folder already holds this person's library. */
export function LibrarySyncDialog({ onClose }: { readonly onClose: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const [found, setFound] = useState<SharedFolder | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);

  const choose = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setFolder(chosen);
    setFound(null);
    setError(null);
    try {
      setFound(await lookInSharedFolder(chosen));
    } catch {
      // A folder that cannot be read is reported when syncing starts.
    }
  };
  const merging = found?.kind === "library";
  // A Space or a page share keeps its folder to itself.
  const taken = found !== null && !merging;
  const start = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!folder || taken || pending) return;
    setPending(true);
    setError(null);
    try {
      // One person, one member: the other computer's member is this one's.
      await shareLibrary(folder, merging ? found.members[0]?.id : null);
      await queryClient.invalidateQueries({ queryKey: SHARES_KEY });
      onClose();
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setPending(false);
    }
  };
  const outsideCloud = folder !== null && cloudName(folder, t) === null;
  // Phone AI tools reach the folder through a Google Drive connector only.
  const phoneNeedsDrive = folder !== null && !outsideCloud && cloudName(folder, t) !== "Google Drive";
  return (
    <dialog
      ref={dialogRef}
      className="share-dialog"
      aria-labelledby="library-sync-title"
      onCancel={(event) => { event.preventDefault(); if (!pending) onClose(); }}
    >
      <form onSubmit={start}>
        {merging ? (
          <>
            <h2 id="library-sync-title">{t("librarySync.mergeTitle")}</h2>
            <p className="library-sync-text">{t("librarySync.mergeText")}</p>
          </>
        ) : (
          <>
            <h2 id="library-sync-title">{t("librarySync.title")}</h2>
            <div className="share-field">
              <div className="share-folder-row">
                <span className="share-folder" data-empty={folder === null}>
                  <FolderIcon />
                  <span>{folder ? shortFolder(folder, t) : t("sharing.noFolder")}</span>
                </span>
                <button type="button" className="share-secondary" onClick={() => void choose()} disabled={pending}>{t("sharing.choose")}</button>
              </div>
              <p className="share-hint">{t("librarySync.folderHint")}</p>
              {taken && <p className="share-warning" role="alert">{t("sharing.alreadyShared")}</p>}
              {!taken && outsideCloud && <p className="share-warning">{t("sharing.notCloud")}</p>}
            </div>
            <dl className="library-sync-what">
              <dt>{t("librarySync.willSync")}</dt><dd>{t("librarySync.willSyncValue")}</dd>
              <dt>{t("librarySync.phoneAi")}</dt><dd>{t(phoneNeedsDrive ? "librarySync.phoneAiNeedsDrive" : "librarySync.phoneAiValue")}</dd>
              <dt>{t("librarySync.wontSync")}</dt><dd>{t("librarySync.wontSyncValue")}</dd>
            </dl>
          </>
        )}
        {error && <p className="share-warning" role="alert">{t("librarySync.failed", { message: error })}</p>}
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose} disabled={pending}>{t("sharing.cancel")}</button>
          <button type="submit" className="share-primary" disabled={!folder || taken || pending}>
            {merging
              ? t(pending ? "librarySync.merging" : "librarySync.merge")
              : t(pending ? "librarySync.starting" : "librarySync.start")}
          </button>
        </div>
      </form>
    </dialog>
  );
}
