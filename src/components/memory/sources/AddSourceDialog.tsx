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

  const addMutation = useMutation({
    mutationFn: ({ sourceType, sourcePath }: { sourceType: "obsidian" | "directory"; sourcePath: string }) =>
      addSource(sourceType, sourcePath),
    onSuccess: (newSource, { sourceType }) => {
      queryClient.invalidateQueries({ queryKey: ["registeredSources"] });
      // The daemon's 30s scheduler follows later vault changes, but it leaves a
      // vault's first import to us so it starts at once even while the
      // ordinary poll waits for the machine to be idle.
      if (sourceType === "obsidian") {
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
    });
  }, [addMutation, detection, path]);

  const canSubmit = path.length > 0 && !detecting && !addMutation.isPending;

  return <SourceDialog title={t("sourceAccess.folderTitle")} onClose={onClose}>
    <p>{t("sourceAccess.folderBody")}</p>
    <label>{t("sourceAccess.folderPath")}<input type="text" value={path} placeholder={t("sourceAccess.chooseFolder")} readOnly /></label>
    <button type="button" onClick={handleBrowse} className="page-editor-action">{t("sourceAccess.browse")}</button>
    {detecting && <p role="status">{t("sourceAccess.scanning")}</p>}
    {detection && !detecting && <p>{detection.isVault ? t("sourceAccess.vaultDetected") : detection.docCount > 0 ? detection.countCapped ? t("vaultConnect.filesFoundCapped") : t("vaultConnect.filesFound", {count:detection.docCount}) : t("vaultConnect.noneFound")}</p>}
    {error && <p role="alert" className="source-dialog-error">{error}</p>}
    <div className="source-dialog-actions">
      <button type="button" className="page-editor-action" onClick={onClose}>{t("sourceAccess.cancel")}</button>
      <button type="button" className="page-editor-action" onClick={handleSubmit} disabled={!canSubmit}>{t(addMutation.isPending ? "sourceAccess.adding" : "sourceAccess.connect")}</button>
    </div>
  </SourceDialog>;
}
