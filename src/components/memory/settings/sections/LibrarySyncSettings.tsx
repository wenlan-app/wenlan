// SPDX-License-Identifier: AGPL-3.0-only
// Settings › General › Other computers: keep the whole library the same on
// this person's other computers through a cloud folder.
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import {
  listShares,
  lookInSharedFolder,
  revealSharedFolder,
  setLibraryMainDevice,
  shareLibrary,
  stopSharingLibrary,
  suggestLibraryFolder,
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

/** Google Drive for desktop, for a computer that has no Google Drive yet. */
const GOOGLE_DRIVE_DOWNLOAD = "https://www.google.com/drive/download/";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function libraryShare(shares: readonly SharedStatus[] | undefined): SharedStatus | undefined {
  return shares?.find((share) => share.kind === "library");
}

/** The line under the row's title once syncing is on. */
function syncLine(share: SharedStatus, t: TFunction, locale: string): string {
  const cloudLabel = cloudName(share.folder, t) ?? shortFolder(share.folder, t);
  if (share.progress && share.progress.total > 0) {
    return `${cloudLabel} · ${t("librarySync.progress", { done: share.progress.done, total: share.progress.total })}`;
  }
  const last = share.last;
  if (!last) return t("sharing.syncing");
  if (last.paused) return t("sharing.paused");
  if (last.counts.skipped > 0) return t("sharing.waiting", { count: last.counts.skipped });
  const when = relativeMs(Date.parse(last.at), locale);
  return `${cloudLabel} · ${t("librarySync.synced", { when })}`;
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
      {share && <NextSteps share={share} t={t} />}
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

const NEXT_DONE_KEY = "wenlan-sync-next-done";

function readNextDone(): boolean {
  try {
    return localStorage.getItem(NEXT_DONE_KEY) === "1";
  } catch {
    return false;
  }
}

/** What is left to do once syncing is on: the other computer (ticked by
 *  itself once it joins) and the phone's AI (Wenlan cannot see that one, so
 *  the person closes the row). */
function NextSteps({ share, t }: { readonly share: SharedStatus; readonly t: TFunction }) {
  const [done, setDone] = useState(readNextDone);
  const [howTo, setHowTo] = useState(false);
  if (done) return null;
  const other = (share.devices ?? []).find((device) => device.id !== share.this_device);
  const phoneOk = cloudName(share.folder, t) === "Google Drive";
  const folderName = share.folder.split(/[\\/]+/).filter(Boolean).pop() ?? share.folder;
  const close = () => {
    try {
      localStorage.setItem(NEXT_DONE_KEY, "1");
    } catch {
      // Shown again next time; nothing is lost.
    }
    setDone(true);
  };
  return (
    <div className="settings-row px-5 py-4">
      <div className="library-sync-next">
        <div className="library-sync-next-copy">
          <div className="library-sync-next-title">{t("librarySync.next")}</div>
          <ul className="library-sync-steps">
            <li>
              <span className="library-sync-dot" data-done={Boolean(other)} aria-hidden="true" />
              <span className="library-sync-who">{t("librarySync.nextOther")}</span>
              <span>{other ? t("librarySync.nextOtherDone", { name: other.name }) : t("librarySync.nextOtherTodo")}</span>
            </li>
            <li>
              <span className="library-sync-dot" aria-hidden="true" />
              <span className="library-sync-who">{t("librarySync.nextPhone")}</span>
              {phoneOk ? (
                <span>
                  {t("librarySync.nextPhoneTodo")}
                  {" · "}
                  <button type="button" className="share-link" onClick={() => setHowTo(true)}>{t("librarySync.howTo")}</button>
                </span>
              ) : (
                <span>{t("librarySync.phoneAiNeedsDrive")}</span>
              )}
            </li>
          </ul>
        </div>
        <Button variant="secondary" size="sm" onClick={close}>{t("librarySync.nextDone")}</Button>
      </div>
      {howTo && <PhoneHowTo folder={folderName} t={t} onClose={() => setHowTo(false)} />}
    </div>
  );
}

function PhoneHowTo({ folder, t, onClose }: { readonly folder: string; readonly t: TFunction; readonly onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  return (
    <dialog
      ref={dialogRef}
      className="share-dialog"
      aria-labelledby="library-sync-phone-title"
      onCancel={(event) => { event.preventDefault(); onClose(); }}
    >
      <form method="dialog" onSubmit={(event) => { event.preventDefault(); onClose(); }}>
        <h2 id="library-sync-phone-title">{t("librarySync.phoneTitle")}</h2>
        <ol className="library-sync-howto">
          <li>{t("librarySync.phoneStep1")}</li>
          <li>{t("librarySync.phoneStep2", { folder })}</li>
        </ol>
        <p className="share-hint">{t("librarySync.phoneInbox")}</p>
        <div className="share-actions">
          <button type="submit" className="share-primary">{t("librarySync.gotIt")}</button>
        </div>
      </form>
    </dialog>
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
  // Where the folder came from: found by itself in this computer's cloud
  // drive, or none found (then the person is told how to get Google Drive).
  const [picked, setPicked] = useState<"google_drive" | "icloud" | "none" | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);

  const lookIn = async (chosen: string) => {
    setFolder(chosen);
    setFound(null);
    setError(null);
    try {
      setFound(await lookInSharedFolder(chosen));
    } catch {
      // A folder that cannot be read is reported when syncing starts.
    }
  };
  const chosenByHand = useRef(false);
  useEffect(() => {
    let live = true;
    suggestLibraryFolder()
      .then(async (suggested) => {
        if (!live || chosenByHand.current) return;
        if (!suggested.folder || !suggested.cloud) {
          setPicked("none");
          return;
        }
        setPicked(suggested.cloud);
        await lookIn(suggested.folder);
      })
      .catch(() => {
        if (live && !chosenByHand.current) setPicked("none");
      });
    return () => {
      live = false;
    };
  }, []);
  // A first sync of a big library takes a while: show how far it is.
  useEffect(() => {
    if (!pending) {
      setProgress(null);
      return;
    }
    const timer = window.setInterval(() => {
      listShares()
        .then((shares) => setProgress(libraryShare(shares)?.progress ?? null))
        .catch(() => undefined);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [pending]);

  const choose = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    chosenByHand.current = true;
    setPicked(null);
    await lookIn(chosen);
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
            {folder && <p className="share-hint">{shortFolder(folder, t)}</p>}
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
                <button type="button" className="share-secondary" onClick={() => void choose()} disabled={pending}>{t(folder ? "librarySync.chooseOther" : "sharing.choose")}</button>
              </div>
              {picked === "google_drive" && <p className="share-hint">{t("librarySync.picked")}</p>}
              {picked === "icloud" && <p className="share-hint">{t("librarySync.pickedIcloud")}</p>}
              {picked === "none" && folder === null && (
                <p className="share-warning">
                  {t("librarySync.noDrive")}{" "}
                  <button type="button" className="share-link" onClick={() => { void Promise.resolve(openExternal(GOOGLE_DRIVE_DOWNLOAD)).catch(() => undefined); }}>{t("librarySync.getDrive")}</button>
                </p>
              )}
              {picked === null && <p className="share-hint">{t("librarySync.folderHint")}</p>}
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
        {pending && progress && progress.total > 0 && (
          <p className="share-hint" role="status">{t("librarySync.progress", { done: progress.done, total: progress.total })}</p>
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
