// SPDX-License-Identifier: AGPL-3.0-only

export interface PdfReadingState {
  readonly page: number;
  readonly scale: string;
  readonly query: string;
}

const MAX_PDF_READING_STATES = 100;
const states = new Map<string, PdfReadingState>();
const zoteroAttachments = new Map<string, string>();

function stateKey(sessionKey: string, documentId: string) {
  return JSON.stringify([sessionKey, documentId]);
}

/** Keeps a bounded, in-memory reading position per open page and attachment. */
export function readPdfReadingState(sessionKey: string | undefined, documentId: string): PdfReadingState | undefined {
  if (!sessionKey) return undefined;
  const key = stateKey(sessionKey, documentId);
  const state = states.get(key);
  if (!state) return undefined;
  states.delete(key);
  states.set(key, state);
  return state;
}

export function savePdfReadingState(sessionKey: string | undefined, documentId: string, state: PdfReadingState): void {
  if (!sessionKey || !Number.isInteger(state.page) || state.page < 1) return;
  const key = stateKey(sessionKey, documentId);
  states.delete(key);
  states.set(key, state);
  while (states.size > MAX_PDF_READING_STATES) states.delete(states.keys().next().value!);
}

function zoteroSelectionKey(sessionKey: string, sourceId: string, space: string | undefined) {
  return JSON.stringify([sessionKey, sourceId, space ?? null]);
}

export function readZoteroAttachmentSelection(sessionKey: string | undefined, sourceId: string, space: string | undefined): string | undefined {
  if (!sessionKey) return undefined;
  const key = zoteroSelectionKey(sessionKey, sourceId, space);
  const attachment = zoteroAttachments.get(key);
  if (!attachment) return undefined;
  zoteroAttachments.delete(key);
  zoteroAttachments.set(key, attachment);
  return attachment;
}

export function saveZoteroAttachmentSelection(sessionKey: string | undefined, sourceId: string, space: string | undefined, attachmentKey: string): void {
  if (!sessionKey || !attachmentKey) return;
  const key = zoteroSelectionKey(sessionKey, sourceId, space);
  zoteroAttachments.delete(key);
  zoteroAttachments.set(key, attachmentKey);
  while (zoteroAttachments.size > MAX_PDF_READING_STATES) zoteroAttachments.delete(zoteroAttachments.keys().next().value!);
}
