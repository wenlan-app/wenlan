// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import { listNewMaterial, listRegisteredSources, type RegisteredSource } from "../../../lib/tauri";
import { ImportOrganize, useOrganizeActions, type ImportedKind } from "../askAi/TurnIntoPages";
import { isNoteApp } from "./sourcePresentation";

/**
 * The import finish for a folder, an Obsidian vault or a Logseq graph.
 *
 * A folder source makes no import batch, so the finish the other imports
 * show never came after one. The add flows remember the folder they just
 * added (for this session only), and the places a user lands afterwards (the
 * Sources library, that folder, Settings → Sources, the first-run checklist)
 * show the same "Turn them into pages" finish once its first read has stored
 * something. "Later" forgets it.
 */
interface AddedFolder {
  readonly id: string;
  readonly kind: ImportedKind;
  /** The most the finish has counted, so a count never goes back down while
   *  pages take items out of the waiting list. */
  readonly seen: number;
}

let added: AddedFolder | null = null;
const listeners = new Set<() => void>();

function publish(next: AddedFolder | null) {
  added = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** What the folder's items are called: notes from a notes app, else files. */
export function folderImportKind(sourceType: string): ImportedKind {
  return isNoteApp(sourceType) ? "note" : "file";
}

/** Call once a folder source is added, before its first read finishes. */
export function rememberAddedFolder(source: Pick<RegisteredSource, "id" | "source_type">): void {
  publish({ id: source.id, kind: folderImportKind(source.source_type), seen: 0 });
}

/** Forget the added folder (all of them, without an id). */
export function forgetAddedFolder(id?: string): void {
  if (id === undefined || added?.id === id) publish(null);
}

function useAddedFolder(): AddedFolder | null {
  return useSyncExternalStore(subscribe, () => added, () => null);
}

const SOURCE_POLL_MS = 3_000;
const MATERIAL_POLL_MS = 15_000;

export interface FolderImportFinish {
  readonly id: string;
  readonly kind: ImportedKind;
  /** Items of this folder's kind stored and searchable now. */
  readonly count: number;
}

/**
 * The folder just added, once its first read has finished and stored at
 * least one item; null before then, or for another folder when `sourceId`
 * names one.
 *
 * The count is how many items of the folder's kind are stored and still
 * waiting for a page (the list the card hands to the AI tool), never more
 * than the files the folder holds. The source's own counters are not used:
 * a folder's counts files handed to document prep, which may still be
 * running, and a vault's counts passages, not notes.
 */
export function useFolderImportFinish(sourceId?: string): FolderImportFinish | null {
  const folder = useAddedFolder();
  const relevant = folder !== null && (sourceId === undefined || sourceId === folder.id) ? folder : null;
  const sources = useQuery({
    queryKey: ["registeredSources"],
    // Read only when called, so screens with no folder added never touch it.
    queryFn: () => listRegisteredSources(),
    enabled: relevant !== null,
    refetchInterval: (query) => {
      const source = query.state.data?.find((item) => item.id === relevant?.id);
      return relevant !== null && source?.last_sync == null ? SOURCE_POLL_MS : false;
    },
  });
  const source = relevant ? sources.data?.find((item) => item.id === relevant.id) : undefined;
  const read = source !== undefined && source.last_sync !== null;
  const material = useQuery({
    queryKey: ["newMaterial"],
    queryFn: () => listNewMaterial(),
    enabled: read,
    refetchInterval: read ? MATERIAL_POLL_MS : false,
  });
  // A folder removed while the finish waits takes the finish with it.
  const removed = relevant !== null && sources.isSuccess && source === undefined;
  useEffect(() => {
    if (removed && relevant) forgetAddedFolder(relevant.id);
  }, [removed, relevant]);

  const stored = read && material.data
    ? Math.min(material.data.kinds?.[relevant!.kind] ?? 0, source.file_count)
    : 0;
  useEffect(() => {
    if (relevant && stored > relevant.seen && added?.id === relevant.id) {
      publish({ ...relevant, seen: stored });
    }
  }, [relevant, stored]);
  if (!relevant || !read) return null;
  const count = Math.max(stored, relevant.seen);
  return count > 0 ? { id: relevant.id, kind: relevant.kind, count } : null;
}

interface FolderImportOrganizeProps {
  /** Only this folder's finish (inside that folder). */
  readonly sourceId?: string;
  /** Inside a box that is already the screen's card: no box of its own. */
  readonly bare?: boolean;
  readonly className?: string;
}

/** The folder import's finish where the user lands after adding it. Inside
 *  the main window only, where the card's actions lead somewhere. */
export function FolderImportOrganize({ sourceId, bare = false, className }: FolderImportOrganizeProps) {
  const finish = useFolderImportFinish(sourceId);
  const actions = useOrganizeActions();
  if (!finish || !actions) return null;
  return (
    <div className={className} data-testid="folder-import-organize">
      <ImportOrganize count={finish.count} kind={finish.kind} bare={bare} onLater={() => forgetAddedFolder(finish.id)} />
    </div>
  );
}
