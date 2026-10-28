// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ArrowsClockwise, ChatsCircle, ClipboardText, DownloadSimple, FileZip } from "@phosphor-icons/react";
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
import { ImportFlow } from "../../ChatImport/ImportFlow";
import SourceDialog from "./SourceDialog";
import { SourceTypeIcon } from "./SourceTypeIcon";
import { rememberAddedFolder } from "./FolderImportOrganize";
import { useTranslation } from "react-i18next";

type ConnectKind = "obsidian" | "logseq" | "folder";
type PickedFolder = { kind: ConnectKind; sourceType: SourceTypeStr; path: string };

export default function AddSourceMenu({ onClose, start = "menu", onQuickChats, title }: {
  onClose: () => void;
  /** "chats" opens straight at importing past AI chats. */
  start?: "menu" | "chats";
  /** Where past AI chats can also come in the quick way (copy a prompt into
   *  the AI, paste its answer back), the chats panel offers both ways. */
  onQuickChats?: () => void;
  /** The menu's title, when the button that opened it named it otherwise. */
  title?: string;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [showWeb, setShowWeb] = useState(false);
  const [showChats, setShowChats] = useState(start === "chats");
  // Quick or full: asked first wherever the quick way exists.
  const [fullChats, setFullChats] = useState(onQuickChats === undefined);
  // A picked folder waits here while the user chooses sync or import-once.
  const [picked, setPicked] = useState<PickedFolder | null>(null);
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
      const sourceType: SourceTypeStr = kind === "logseq" ? "logseq" : detection.sourceType;
      return { kind, sourceType, path };
    },
    onSuccess: (folder) => { if (folder) setPicked(folder); },
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

  const busy = upload.isPending || connect.isPending || register.isPending;
  if (picked) return <SourceDialog key="follow" title={t("sourceAccess.followTitle")} onClose={onClose}>
    <p className="source-follow-path">{picked.path}</p>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => register.mutate({ folder: picked, followChanges: true })}>
      <ArrowsClockwise aria-hidden="true" size={20} weight="regular" data-resource-icon="sync" /><span className="source-add-option-copy"><strong>{t("sourceAccess.followSync")}</strong><span>{t("sourceAccess.followSyncHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => register.mutate({ folder: picked, followChanges: false })}>
      <DownloadSimple aria-hidden="true" size={20} weight="regular" data-resource-icon="import-once" /><span className="source-add-option-copy"><strong>{t("sourceAccess.followOnce")}</strong><span>{t("sourceAccess.followOnceHint")}</span></span>
    </button>
    {register.isPending && <p role="status">{t("sourceAccess.connecting")}</p>}
    {failure && <div role="alert" className="source-dialog-error">{t("sourceAccess.addFailed")}<p>{failure}</p></div>}
  </SourceDialog>;
  if (showWeb) return <AddWebSourceDialog onClose={onClose} />;
  // Past AI chats use the same import as Settings, so a new user can bring
  // earlier decisions in from the home without hunting for it.
  // The two ways are offered side by side: one is done in a minute and
  // brings what the AI remembers; the other brings every chat but waits for
  // an email.
  if (showChats && !fullChats && onQuickChats) return <SourceDialog title={t("sourceAccess.chatHistory")} lead={t("sourceAccess.chatHistoryLead")} onClose={onClose}>
    <button type="button" className="source-add-option" data-testid="source-chats-quick" onClick={() => { onClose(); onQuickChats(); }}>
      <ClipboardText aria-hidden="true" size={20} weight="regular" data-resource-icon="quick" /><span className="source-add-option-copy"><strong>{t("sourceAccess.chatQuick")}</strong><span>{t("sourceAccess.chatQuickHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" data-testid="source-chats-full" onClick={() => setFullChats(true)}>
      <FileZip aria-hidden="true" size={20} weight="regular" data-resource-icon="zip" /><span className="source-add-option-copy"><strong>{t("sourceAccess.chatFull")}</strong><span>{t("sourceAccess.chatFullHint")}</span></span>
    </button>
  </SourceDialog>;
  if (showChats) return <SourceDialog title={t("sourceAccess.chatHistory")} onClose={onClose}>
    {/* The export arrives by email, sometimes much later, so the dialog says
        so up front instead of leaving the user waiting at the drop zone. */}
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
  </SourceDialog>;
  return <SourceDialog title={title ?? t("sourceAccess.addTitle")} lead={t("sourceAccess.addLead")} onClose={onClose}>
    {!ready && <div role="status"><p>{t("sourceAccess.upgrade")}</p><button className="page-editor-action" type="button" onClick={() => openFile("https://wenlan.app")}>{t("sourceAccess.update")}</button></div>}
    {/* Past AI chats first: everyone who uses an AI tool has them. Then an
        Obsidian vault, a folder, files; a link with its text, Logseq last. */}
    <button type="button" className="source-add-option" disabled={busy} onClick={() => setShowChats(true)}>
      <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /><span className="source-add-option-copy"><strong>{t("sourceAccess.chatHistory")}</strong><span>{t("sourceAccess.chatHistoryHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => connect.mutate("obsidian")}>
      <SourceTypeIcon kind="obsidian" /><span className="source-add-option-copy"><strong>{t("sourceAccess.obsidian")}</strong><span>{t("sourceAccess.obsidianHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => connect.mutate("folder")}>
      <SourceTypeIcon kind="folder" /><span className="source-add-option-copy"><strong>{t("sourceAccess.folder")}</strong><span>{t("sourceAccess.folderHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => upload.mutate()}>
      <SourceTypeIcon kind="file" /><span className="source-add-option-copy"><strong>{t(upload.isPending ? "sourceAccess.adding" : "sourceAccess.files")}</strong><span>{t("sourceAccess.filesHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => setShowWeb(true)}>
      <SourceTypeIcon kind="webpage" /><span className="source-add-option-copy"><strong>{t("sourceAccess.web")}</strong><span>{t("sourceAccess.webHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => connect.mutate("logseq")}>
      <SourceTypeIcon kind="logseq" /><span className="source-add-option-copy"><strong>{t("sourceAccess.logseq")}</strong><span>{t("sourceAccess.logseqHint")}</span></span>
    </button>
    {connect.isPending && <p role="status">{t("sourceAccess.connecting")}</p>}
    {failure && <div role="alert" className="source-dialog-error">{t("sourceAccess.addFailed")}<p>{failure}</p></div>}
  </SourceDialog>;
}
