import { useState, useCallback } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { addSource, syncRegisteredSource } from "../../../lib/tauri";
import { detectVault, type VaultDetection } from "../../../lib/vaultDetection";
import SourceDialog from "./SourceDialog";
import { toast } from "sonner";

interface Props {
  onClose: () => void;
  onSuccess: () => void;
}

export default function AddSourceDialog({ onClose, onSuccess }: Props) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [path, setPath] = useState("");
  const [detection, setDetection] = useState<VaultDetection | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [followChanges, setFollowChanges] = useState(true);

  const addMutation = useMutation({
    mutationFn: ({ sourceType, sourcePath, follow }: { sourceType: VaultDetection["sourceType"]; sourcePath: string; follow: boolean }) =>
      addSource(sourceType, sourcePath, follow),
    onSuccess: (newSource, { sourceType, follow }) => {
      queryClient.invalidateQueries({ queryKey: ["registeredSources"] });
      // The daemon's 30s scheduler follows later vault changes, but it leaves a
      // vault's (or Logseq graph's) first import to us so it starts at once
      // even while the ordinary poll waits for the machine to be idle. An
      // import-once source is never polled, so its only automatic read is this one.
      if (sourceType === "obsidian" || sourceType === "logseq" || !follow) {
        syncRegisteredSource(newSource.id).then(() => {
          queryClient.invalidateQueries({ queryKey: ["registeredSources"] });
        }).catch(() => toast(t("sourceAccess.addFailed")));
      }
      onSuccess();
    },
    onError: (err) => {
      setError(err instanceof Error ? err.message : String(err));
    },
  });

  const handleBrowse = useCallback(async () => {
    try {
      const selected = await openDialog({ directory: true, multiple: false });
      if (!selected) return;
      const selectedPath = typeof selected === "string" ? selected : selected;
      setPath(selectedPath);
      setDetecting(true);
      setError(null);
      setDetection(null);

      const d = await detectVault(selectedPath);
      setDetection(d);
      setDetecting(false);
    } catch {
      setDetecting(false);
      setError(t("sourceAccess.folderPickFailed"));
    }
  }, [t]);

  const handleSubmit = useCallback(() => {
    setError(null);
    addMutation.mutate({
      sourceType: detection?.sourceType ?? "directory",
      sourcePath: path,
      follow: followChanges,
    });
  }, [addMutation, detection, path, followChanges]);

  const canSubmit = path.length > 0 && !detecting && !addMutation.isPending;

  return <SourceDialog title={t("sourceAccess.folderTitle")} onClose={onClose}>
    <p>{t("sourceAccess.folderBody")}</p>
    <label>{t("sourceAccess.folderPath")}<input type="text" value={path} placeholder={t("sourceAccess.chooseFolder")} readOnly /></label>
    <button type="button" onClick={handleBrowse} className="page-editor-action">{t("sourceAccess.browse")}</button>
    {detecting && <p role="status">{t("sourceAccess.scanning")}</p>}
    {detection && !detecting && <p>{detection.isVault ? t("sourceAccess.vaultDetected") : detection.isLogseqGraph ? t("sourceAccess.logseqDetected") : detection.docCount > 0 ? detection.countCapped ? t("vaultConnect.filesFoundCapped") : t("vaultConnect.filesFound", {count:detection.docCount}) : t("vaultConnect.noneFound")}</p>}
    <fieldset className="source-follow-choice">
      <legend>{t("sourceAccess.followTitle")}</legend>
      <label><input type="radio" name="source-follow" checked={followChanges} onChange={() => setFollowChanges(true)} /><span><strong>{t("sourceAccess.followSync")}</strong> {t("sourceAccess.followSyncHint")}</span></label>
      <label><input type="radio" name="source-follow" checked={!followChanges} onChange={() => setFollowChanges(false)} /><span><strong>{t("sourceAccess.followOnce")}</strong> {t("sourceAccess.followOnceHint")}</span></label>
    </fieldset>
    {error && <p role="alert" className="source-dialog-error">{error}</p>}
    <div className="source-dialog-actions">
      <button type="button" className="page-editor-action" onClick={onClose}>{t("sourceAccess.cancel")}</button>
      <button type="button" className="page-editor-action" onClick={handleSubmit} disabled={!canSubmit}>{t(addMutation.isPending ? "sourceAccess.adding" : "sourceAccess.connect")}</button>
    </div>
  </SourceDialog>;
}
