// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useReducer, type SetStateAction } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import type { IndexedFileInfo } from "../../../lib/tauri";

interface InspectorSession {
  infoOpen: boolean;
  canvasOpen: boolean;
  pdfPreview: IndexedFileInfo | null;
  pdfPreviewOpen: boolean;
}

// UI-only, bounded to the current application's query client. No source data
// or preferences are written back when the reader switches notes.
const sessions = new WeakMap<QueryClient, Map<string, InspectorSession>>();
const empty = (): InspectorSession => ({ infoOpen: false, canvasOpen: false, pdfPreview: null, pdfPreviewOpen: false });

export function usePageInspectorSession(key: string) {
  const client = useQueryClient();
  const [, render] = useReducer((version: number) => version + 1, 0);
  let cache = sessions.get(client);
  if (!cache) { cache = new Map(); sessions.set(client, cache); }
  const state = cache.get(key) ?? empty();
  const update = useCallback(<K extends keyof InspectorSession>(field: K, value: SetStateAction<InspectorSession[K]>) => {
    const cache = sessions.get(client)!;
    const previous = cache.get(key) ?? empty();
    const nextValue = typeof value === "function"
      ? (value as (value: InspectorSession[K]) => InspectorSession[K])(previous[field]) : value;
    cache.delete(key);
    cache.set(key, { ...previous, [field]: nextValue });
    if (cache.size > 64) cache.delete(cache.keys().next().value!);
    render();
  }, [client, key]);
  const setInfoOpen = useCallback((value: SetStateAction<boolean>) => update("infoOpen", value), [update]);
  const setCanvasOpen = useCallback((value: SetStateAction<boolean>) => update("canvasOpen", value), [update]);
  const setPdfPreview = useCallback((value: SetStateAction<IndexedFileInfo | null>) => update("pdfPreview", value), [update]);
  const setPdfPreviewOpen = useCallback((value: SetStateAction<boolean>) => update("pdfPreviewOpen", value), [update]);
  return { ...state, setInfoOpen, setCanvasOpen, setPdfPreview, setPdfPreviewOpen };
}
