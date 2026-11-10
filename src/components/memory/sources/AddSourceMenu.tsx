// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Books, ChatsCircle } from "@phosphor-icons/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  uploadSourceFile,
  getDaemonVersion,
  daemonMeetsFloor,
  openFile,
  addSource,
  syncRegisteredSource,
  type SourceTypeStr,
} from "../../../lib/tauri";
import { detectVault } from "../../../lib/vaultDetection";
import AddWebSourceDialog from "./AddWebSourceDialog";
import ZoteroSourceDialog from "./ZoteroSourceDialog";
import LinkVaultDialog from "./LinkVaultDialog";
import { ImportFlow } from "../../ChatImport/ImportFlow";
import SourceDialog from "./SourceDialog";
import { SourceTypeIcon } from "./SourceTypeIcon";
import { rememberAddedFolder } from "./FolderImportOrganize";
import { useTranslation } from "react-i18next";

type ConnectKind = "obsidian" | "logseq" | "folder";
type PickedFolder = { kind: ConnectKind; sourceType: SourceTypeStr; path: string };

export default function AddSourceMenu({ onClose, start = "menu", onQuickChats, title }: {
  onClose: () => void;
  /** "chats" opens straight at importing past AI chats; "folder" opens the
   *  folder picker at once (an Obsidian vault is recognised from the folder). */
  start?: "menu" | "chats" | "folder";
  /** Where past AI chats can also come in the quick way (copy a prompt into
   *  the AI, paste its answer back), the chats panel offers it first and the
   *  full export as a link under it. */
  onQuickChats?: () => void;
  /** The menu's title, when the button that opened it named it otherwise. */
  title?: string;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [showWeb, setShowWeb] = useState(false);
  const [showZotero, setShowZotero] = useState(false);
  // An Obsidian vault is linked into the Wiki, not read as a source (UX-ADD-3).
  // A folder picked from the folder entry that is an Obsidian vault goes the
  // same way, with that vault already chosen.
  const [showLinkVault, setShowLinkVault] = useState(false);
  const [linkPath, setLinkPath] = useState<string | undefined>(undefined);
  const [showChats, setShowChats] = useState(start === "chats");
  const [showFullChats, setShowFullChats] = useState(false);
  // A failure to add a source is shown here in the dialog, next to the choice
  // that failed, rather than as a toast that disappears.
  const [failure, setFailure] = useState<string | null>(null);
  const fail = (error: unknown) => setFailure(error instanceof Error ? error.message : String(error));
  const { data: version } = useQuery({ queryKey: ["daemonVersion"], queryFn: getDaemonVersion });
  const ready = version === undefined ? true : daemonMeetsFloor(version);

  function refreshSources() {
    void qc.invalidateQueries({ queryKey: ["registeredSources"] });
    void qc.invalidateQueries({ queryKey: ["sourceDir"] });
    void qc.invalidateQueries({ queryKey: ["indexedFiles"] });
  }

  const upload = useMutation({
    onMutate: () => setFailure(null),
    mutationFn: async () => {
      const picked = await openDialog({
        title: t("sourceAccess.files"), directory: false, multiple: false,
        filters: [{ name: t("sourceAccess.pickerFilter"), extensions: ["pdf", "md", "txt"] }],
      });
      if (!picked || typeof picked !== "string") return null;
      await uploadSourceFile(picked);
      return picked.replace(/\\/g, "/").split("/").pop() ?? picked;
    },
    onSuccess: (name) => {
      if (!name) return;
      refreshSources();
      toast(t("sourceAccess.added"), { description: t("sourceAccess.addedBody", { name }) });
      onClose();
    },
    onError: fail,
  });

  // Step one, for any kind: pick and validate the folder. Step two
  // (`register`) asks whether to keep it in sync or import it once.
  const connect = useMutation({
    onMutate: () => setFailure(null),
    mutationFn: async (kind: ConnectKind) => {
      const path = await openDialog({
        title: t(kind === "folder" ? "sourceAccess.folder" : `sourceAccess.${kind}`),
        directory: true, multiple: false,
      });
      if (!path || typeof path !== "string") return null;
      const detection = await detectVault(path);
      if (kind !== "folder" && detection.unreadable) throw new Error(t("sourceAccess.folderPickFailed"));
      if (kind === "obsidian" && !detection.isVault) throw new Error(t("sourceAccess.notVault"));
      if (kind === "logseq" && !detection.isLogseqGraph) throw new Error(t("sourceAccess.notLogseqGraph"));
      // One folder entry for every kind: a Logseq graph is recognised from
      // the folder the same way an Obsidian vault is.
      const sourceType: SourceTypeStr = kind === "logseq" || (detection.isLogseqGraph && detection.sourceType !== "obsidian") ? "logseq" : detection.sourceType;
      return { kind, sourceType, path };
    },
    onSuccess: (folder) => {
      if (!folder) return;
      if (folder.sourceType === "obsidian") {
        setLinkPath(folder.path);
        setShowLinkVault(true);
        return;
      }
      // Kept in sync without asking: a folder you add is one you keep using.
      register.mutate({ folder, followChanges: true });
    },
    onError: fail,
  });

  const register = useMutation({
    onMutate: () => setFailure(null),
    mutationFn: ({ folder, followChanges }: { folder: PickedFolder; followChanges: boolean }) =>
      addSource(folder.sourceType, folder.path, followChanges),
    onSuccess: (source) => {
      refreshSources();
      // Where the user lands next shows the import finish once the first
      // read has stored something.
      rememberAddedFolder(source);
      // A failed first read updates this same toast instead of stacking a second.
      const toastId = `source-connected-${source.id}`;
      toast(t("sourceAccess.connected"), { id: toastId, description: source.path });
      // Both choices read the source once now; only a synced source is
      // checked again in the background.
      void syncRegisteredSource(source.id).then(refreshSources).catch((error) => {
        console.warn("[sources] first update failed", error);
        toast(t("sourceAccess.connected"), { id: toastId, description: t("sourceAccess.connectedUpdateFailed") });
        refreshSources();
      });
      onClose();
    },
    onError: fail,
  });

  // Opened from the welcome screen's folder row: go straight to the picker.
  // Cancelling it leaves the menu, so the other ways in are still there.
  const pickedOnOpen = useRef(false);
  useEffect(() => {
    if (start !== "folder" || pickedOnOpen.current) return;
    pickedOnOpen.current = true;
    connect.mutate("folder");
  }, [start, connect]);

  const busy = upload.isPending || connect.isPending || register.isPending;
  if (showWeb) return <AddWebSourceDialog onClose={onClose} />;
  if (showZotero) return <ZoteroSourceDialog onClose={onClose} />;
  if (showLinkVault) return <LinkVaultDialog onClose={onClose} initialPath={linkPath} />;
  // Past AI chats: where the quick paste exists it comes first, because the
  // full export arrives by email, sometimes a day later. The full history is
  // the link under it; without the quick way it is shown at once.
  if (showChats) return <SourceDialog title={t("sourceAccess.chatHistory")} onClose={onClose}>
    {onQuickChats && !showFullChats ? <>
      <button type="button" className="source-add-option" data-testid="source-chats-quick" onClick={() => { onClose(); onQuickChats(); }}>
        <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /><span className="source-add-option-copy"><strong>{t("sourceAccess.chatQuickTitle")}</strong><span>{t("sourceAccess.chatQuickHint")}</span></span>
      </button>
      <p className="source-add-more">
        <button type="button" className="source-add-more-link" data-testid="source-chats-full" onClick={() => setShowFullChats(true)}>
          {t("sourceAccess.chatFullLink")}
        </button>
      </p>
    </> : <>
      {/* The export arrives by email, sometimes much later, so the dialog
          says so up front instead of leaving the user waiting at the drop zone. */}
      <ol className="source-chat-steps">
        <li>
          <span className="source-chat-step-number" aria-hidden="true">1</span>
          <div className="source-chat-step-copy">
            <strong>{t("sourceAccess.chatStepExport")}</strong>
            <dl className="source-chat-export-steps">
              <div><dt>{t("settings.sources.chatgptName")}</dt><dd>{t("settings.sources.chatgptExportSteps")}</dd></div>
              <div><dt>{t("settings.sources.claudeName")}</dt><dd>{t("settings.sources.claudeExportSteps")}</dd></div>
            </dl>
          </div>
        </li>
        <li>
          <span className="source-chat-step-number" aria-hidden="true">2</span>
          <div className="source-chat-step-copy">
            <strong>{t("sourceAccess.chatStepWait")}</strong>
            <span>{t("sourceAccess.chatStepWaitBody")}</span>
          </div>
        </li>
        <li>
          <span className="source-chat-step-number" aria-hidden="true">3</span>
          <div className="source-chat-step-copy">
            <strong>{t("sourceAccess.chatStepDrop")}</strong>
            <span>{t("sourceAccess.chatStepDropBody")}</span>
          </div>
        </li>
      </ol>
      <ImportFlow onLeave={onClose} />
    </>}
  </SourceDialog>;
  return <SourceDialog title={title ?? t("sourceAccess.addTitle")} lead={t("sourceAccess.addLead")} onClose={onClose}>
    {!ready && <div role="status"><p>{t("sourceAccess.upgrade")}</p><button className="page-editor-action" type="button" onClick={() => openFile("https://wenlan.app")}>{t("sourceAccess.update")}</button></div>}
    {/* Four ways in, by what the person has: past AI chats, a folder (an
        Obsidian vault or Logseq graph is recognised from it), files, and a
        Zotero library. Saving a link is the small link under them. */}
    <button type="button" className="source-add-option" disabled={busy} onClick={() => setShowChats(true)}>
      <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /><span className="source-add-option-copy"><strong>{t("sourceAccess.chatHistory")}</strong><span>{t("sourceAccess.chatHistoryHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => connect.mutate("folder")}>
      <SourceTypeIcon kind="folder" /><span className="source-add-option-copy"><strong>{t("sourceAccess.folder")}</strong><span>{t("sourceAccess.folderHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => upload.mutate()}>
      <SourceTypeIcon kind="file" /><span className="source-add-option-copy"><strong>{t(upload.isPending ? "sourceAccess.adding" : "sourceAccess.files")}</strong><span>{t("sourceAccess.filesHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => setShowZotero(true)}>
      <Books aria-hidden="true" size={20} weight="regular" data-resource-icon="zotero" /><span className="source-add-option-copy"><strong>{t("sourceAccess.zotero")}</strong><span>{t("sourceAccess.zoteroHint")}</span></span>
    </button>
    <p className="source-add-more">
      <button type="button" className="source-add-more-link" data-testid="source-add-link" disabled={busy} onClick={() => setShowWeb(true)}>
        {t("sourceAccess.orSaveLink")}
      </button>
    </p>
    {connect.isPending && <p role="status">{t("sourceAccess.connecting")}</p>}
    {failure && <div role="alert" className="source-dialog-error">{t("sourceAccess.addFailed")}<p>{failure}</p></div>}
  </SourceDialog>;
}
