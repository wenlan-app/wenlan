// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { IndexedFileInfo } from "../../../lib/tauri";
import PageInfoDrawer from "../page/PageInfoDrawer";
import { SourceDocumentReader } from "./SourceDocumentReader";
import "./source-access.css";

export default function SourceDocumentPreview({ file, onClose, space, onOpenSource, onRegisterFlush }: {
  file: IndexedFileInfo;
  onClose: () => void;
  space?: string;
  onOpenSource?: () => void;
  onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
}) {
  const { t } = useTranslation();
  const flushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    flushRef.current = flush;
    onRegisterFlush?.(flush);
  }, [onRegisterFlush]);
  const handleClose = async () => {
    if (flushRef.current && !await flushRef.current()) return;
    onClose();
  };
  const handleOpenSource = async () => {
    if (flushRef.current && !await flushRef.current()) return;
    onOpenSource?.();
  };
  return <PageInfoDrawer open docked inspector defaultWidth={file.source_agent === "zotero" || file.source_id.startsWith("zotero::") ? 560 : undefined} title={file.title} closeLabel={t("sourceAccess.close")} onClose={() => { void handleClose(); }}
    headerActions={onOpenSource && <button type="button" className="page-editor-action source-preview-open" onClick={() => { void handleOpenSource(); }}>
      {t("sourceAccess.goToSource")}
    </button>}>
    <div className="source-document-preview"><SourceDocumentReader file={file} space={space} onRegisterFlush={onRegisterFlush ? registerFlush : undefined} /></div>
  </PageInfoDrawer>;
}
