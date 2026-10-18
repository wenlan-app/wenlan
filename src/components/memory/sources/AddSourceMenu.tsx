// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ChatsCircle } from "@phosphor-icons/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  uploadSourceFile,
  getDaemonVersion,
  daemonMeetsFloor,
  openFile,
  addSource,
  syncRegisteredSource,
} from "../../../lib/tauri";
import { detectVault } from "../../../lib/vaultDetection";
import AddWebSourceDialog from "./AddWebSourceDialog";
import { ImportFlow } from "../../ChatImport/ImportFlow";
import SourceDialog from "./SourceDialog";
import { SourceTypeIcon } from "./SourceTypeIcon";
import { useTranslation } from "react-i18next";

export default function AddSourceMenu({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [showWeb, setShowWeb] = useState(false);
  const [showChats, setShowChats] = useState(false);
  const { data: version } = useQuery({ queryKey: ["daemonVersion"], queryFn: getDaemonVersion });
  const ready = version === undefined ? true : daemonMeetsFloor(version);

  function refreshSources() {
    void qc.invalidateQueries({ queryKey: ["registeredSources"] });
    void qc.invalidateQueries({ queryKey: ["sourceDir"] });
    void qc.invalidateQueries({ queryKey: ["indexedFiles"] });
  }

  const upload = useMutation({
    mutationFn: async () => {
      const picked = await openDialog({
        title: t("sourceAccess.files"), directory: false, multiple: false,
        filters: [{ name: "Documents", extensions: ["pdf", "md", "txt"] }],
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
    onError: (error) => toast(t("sourceAccess.addFailed"), { description: String(error) }),
  });

  const connect = useMutation({
    mutationFn: async (kind: "obsidian" | "folder") => {
      const picked = await openDialog({
        title: t(kind === "obsidian" ? "sourceAccess.obsidian" : "sourceAccess.folder"),
        directory: true, multiple: false,
      });
      if (!picked || typeof picked !== "string") return null;
      const detection = await detectVault(picked);
      if (kind === "obsidian" && detection.unreadable) throw new Error(t("sourceAccess.folderPickFailed"));
      if (kind === "obsidian" && !detection.isVault) throw new Error(t("sourceAccess.notVault"));
      return addSource(detection.sourceType, picked);
    },
    onSuccess: (source) => {
      if (!source) return;
      refreshSources();
      toast(t("sourceAccess.connected"), { description: source.path });
      // Start the first index for either connector. Later Directory checks are
      // automatic; Obsidian checks remain manual until the backend supports them.
      void syncRegisteredSource(source.id).then(refreshSources).catch((error) => {
        toast(t("sourceAccess.connectedUpdateFailed"), { description: String(error) });
        refreshSources();
      });
      onClose();
    },
    onError: (error) => toast(t("sourceAccess.addFailed"), { description: String(error) }),
  });

  const busy = upload.isPending || connect.isPending;
  if (showWeb) return <AddWebSourceDialog onClose={onClose} />;
  // Past AI chats use the same import as Settings, so a new user can bring
  // earlier decisions in from the home without hunting for it.
  if (showChats) return <SourceDialog title={t("sourceAccess.chatHistory")} onClose={onClose}>
    <p>{t("sourceAccess.chatHistoryBody")}</p>
    <ul className="source-chat-export-steps">
      <li><strong>{t("settings.sources.chatgptName")}</strong> {t("settings.sources.chatgptExportSteps")}</li>
      <li><strong>{t("settings.sources.claudeName")}</strong> {t("settings.sources.claudeExportSteps")}</li>
    </ul>
    <ImportFlow />
  </SourceDialog>;
  return <SourceDialog title={t("sourceAccess.addTitle")} onClose={onClose}>
    {!ready && <div role="status"><p>{t("sourceAccess.upgrade")}</p><button className="page-editor-action" type="button" onClick={() => openFile("https://wenlan.app")}>{t("sourceAccess.update")}</button></div>}
    <button type="button" className="source-add-option source-add-option--vault" disabled={busy} onClick={() => connect.mutate("obsidian")}>
      <SourceTypeIcon kind="obsidian" /><span className="source-add-option-copy"><strong>{t("sourceAccess.obsidian")}</strong><span>{t("sourceAccess.obsidianHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => setShowChats(true)}>
      <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /><span className="source-add-option-copy"><strong>{t("sourceAccess.chatHistory")}</strong><span>{t("sourceAccess.chatHistoryHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => upload.mutate()}>
      <SourceTypeIcon kind="file" /><span className="source-add-option-copy"><strong>{t(upload.isPending ? "sourceAccess.adding" : "sourceAccess.files")}</strong><span>{t("sourceAccess.filesHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => setShowWeb(true)}>
      <SourceTypeIcon kind="webpage" /><span className="source-add-option-copy"><strong>{t("sourceAccess.web")}</strong><span>{t("sourceAccess.webHint")}</span></span>
    </button>
    <button type="button" className="source-add-option" disabled={busy} onClick={() => connect.mutate("folder")}>
      <SourceTypeIcon kind="folder" /><span className="source-add-option-copy"><strong>{t("sourceAccess.folder")}</strong><span>{t("sourceAccess.folderHint")}</span></span>
    </button>
    {connect.isPending && <p role="status">{t("sourceAccess.connecting")}</p>}
  </SourceDialog>;
}
