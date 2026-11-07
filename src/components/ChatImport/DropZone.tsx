import { useCallback, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import { Button } from "../memory/settings/primitives";

interface DropZoneProps {
  /** Called when a file is drag-dropped (receives a File object). */
  onFileSelected: (file: File) => void;
  /** Called when a file is picked via the native dialog (receives a path string — no temp file needed). */
  onPathSelected?: (path: string) => void;
  /** Prevents a second import while the current file is being read or uploaded. */
  disabled?: boolean;
}

export function DropZone({ onFileSelected, onPathSelected, disabled = false }: DropZoneProps) {
  const { t } = useTranslation();
  const [isDragging, setIsDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    if (disabled) return;
    setIsDragging(true);
  }, [disabled]);

  const handleDragLeave = useCallback(() => {
    setIsDragging(false);
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragging(false);
      setError(null);
      if (disabled) return;
      const file = e.dataTransfer.files[0];
      if (!file) return;
      if (!file.name.toLowerCase().endsWith(".zip")) {
        setError(t("chatImport.dropZone.zipError"));
        return;
      }
      onFileSelected(file);
    },
    [disabled, onFileSelected, t],
  );

  const handlePickFile = useCallback(async () => {
    if (disabled) return;
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: t("chatImport.dropZone.zipFilter"), extensions: ["zip"] }],
      });
      if (!selected || typeof selected !== "string") return; // user cancelled
      const path = selected;
      setError(null);
      if (onPathSelected) {
        onPathSelected(path);
      } else {
        // Fallback: no onPathSelected handler — report as error
        setError(t("chatImport.dropZone.noPathHandler"));
      }
    } catch (e) {
      setError(t("chatImport.dropZone.pickerFailed", { error: String(e) }));
    }
  }, [disabled, onPathSelected, t]);

  return (
    <div
      data-testid="chat-import-drop-zone"
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
      aria-busy={disabled}
      style={{
        border: `1.5px dashed ${isDragging ? "var(--mem-accent-indigo)" : "var(--mem-border)"}`,
        borderRadius: "var(--mem-r-10)",
        padding: "28px 20px",
        textAlign: "center",
        transition: "all 0.2s ease",
        background: isDragging ? "var(--mem-indigo-bg)" : "transparent",
        cursor: "default",
      }}
    >
      {/* Upload icon */}
      <div
        style={{
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          width: 36,
          height: 36,
          borderRadius: "var(--mem-r-8)",
          background: isDragging ? "var(--mem-accent-indigo)" : "var(--mem-hover-strong)",
          color: isDragging ? "var(--mem-text-on-accent)" : "var(--mem-text-tertiary)",
          transition: "all 0.2s ease",
          marginBottom: 12,
        }}
      >
        <svg width="18" height="18" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" />
        </svg>
      </div>

      <div
        style={{
          fontFamily: "var(--mem-font-body)",
          fontSize: "var(--mem-text-base)",
          fontWeight: 500,
          color: "var(--mem-text)",
          marginBottom: 4,
        }}
      >
        {t("chatImport.dropZone.title")}
      </div>

      <div
        style={{
          fontFamily: "var(--mem-font-body)",
          fontSize: "var(--mem-text-xs)",
          color: "var(--mem-text-tertiary)",
          marginBottom: 14,
        }}
      >
        {t("chatImport.dropZone.subtitle")}
      </div>

      <Button type="button" variant="secondary" size="sm" onClick={handlePickFile} disabled={disabled}>
        {t("chatImport.dropZone.chooseFile")}
      </Button>

      {error && (
        <p
          role="alert"
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-xs)",
            color: "var(--mem-status-danger-text)",
            marginTop: 12,
            lineHeight: "1.5",
          }}
        >
          {error}
        </p>
      )}
    </div>
  );
}
