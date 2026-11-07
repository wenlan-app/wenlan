// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useContext, useMemo, useState, type FocusEvent, type PointerEvent, type ReactNode } from "react";
import {
  WorkspaceDocumentToolsHostContext,
  WorkspaceNoteGroupContext,
  WorkspacePaneHostContext,
  SharedWikiInspectorContext,
} from "../navigation/WorkspacePaneHost";
import "./NoteGroupFrame.css";

export interface NoteGroupFrameProps {
  readonly id: string;
  readonly label: string;
  readonly tabs: ReactNode;
  readonly children: ReactNode;
  readonly onGlobalToolsHost?: (host: HTMLDivElement | null) => void;
  readonly onFocus?: () => void;
  readonly contentId?: string;
}

/** One independently focused note column and its inspector host. */
export function NoteGroupFrame({ id, label, tabs, children, onFocus, contentId, onGlobalToolsHost }: NoteGroupFrameProps) {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [paneHost, setPaneHost] = useState<HTMLDivElement | null>(null);
  const [toolsHost, setToolsHost] = useState<HTMLDivElement | null>(null);
  const sharedInspector = useContext(SharedWikiInspectorContext);
  const assignElement = useCallback((node: HTMLDivElement | null) => setElement(node), []);
  const notifyFocus = (_event: FocusEvent<HTMLDivElement> | PointerEvent<HTMLDivElement>) => onFocus?.();
  const group = useMemo(() => ({ id, element }), [id, element]);

  return (
    <WorkspaceNoteGroupContext.Provider value={group}>
      <WorkspacePaneHostContext.Provider value={paneHost}>
        <WorkspaceDocumentToolsHostContext.Provider value={sharedInspector?.activeGroupId === id && sharedInspector.toolsElement ? sharedInspector.toolsElement : toolsHost}>
          <div className="note-group-frame" data-note-group-id={id} ref={assignElement}
            onFocusCapture={notifyFocus} onPointerDownCapture={notifyFocus}>
            <section aria-label={label} className="note-group-document">
              <header className="note-group-header" data-tauri-drag-region>
                <div className="note-group-tabs" data-tauri-drag-region>{tabs}</div>
                {onGlobalToolsHost && <div className="note-group-global-tools" ref={onGlobalToolsHost} />}
                <div aria-hidden={sharedInspector ? true : undefined} className={`note-group-document-tools${sharedInspector ? " note-group-document-tools--shared" : ""}`} ref={setToolsHost} />
              </header>
              <div className="wiki-workspace-content note-group-content" id={contentId ?? `note-group-${id}-content`}>
                {children}
              </div>
            </section>
            <div className="note-group-inspector-host" ref={setPaneHost} />
          </div>
        </WorkspaceDocumentToolsHostContext.Provider>
      </WorkspacePaneHostContext.Provider>
    </WorkspaceNoteGroupContext.Provider>
  );
}

export default NoteGroupFrame;
