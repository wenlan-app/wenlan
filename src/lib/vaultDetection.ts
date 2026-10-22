// SPDX-License-Identifier: AGPL-3.0-only
import { readDir } from "@tauri-apps/plugin-fs";

export const MAX_DEPTH = 6;
export const MAX_ENTRIES = 5000;

// Validity follows the SOURCE TYPE the daemon will enforce (spec §3, council
// change e): obsidian sources index markdown only (has_any_markdown); plain
// directory sources ingest md/txt/pdf (daemon directory.rs filter). A Logseq
// graph is read like a vault, from its pages/ and journals/ folders only.
const OBSIDIAN_EXTENSIONS = [".md"];
const LOGSEQ_PAGE_DIRS = ["pages", "journals"];
const DIRECTORY_EXTENSIONS = [".md", ".txt", ".pdf"];

export interface VaultDetection {
  isVault: boolean;
  /** A `logseq/` folder next to `pages/` or `journals/`. A folder can be both
   *  an Obsidian vault and a Logseq graph; `sourceType` then prefers Obsidian. */
  isLogseqGraph: boolean;
  sourceType: "obsidian" | "logseq" | "directory";
  docCount: number;
  countCapped: boolean;
  hasValidDoc: boolean;
  unreadable: boolean;
}

/** Recursive, bounded walk. Never a reason to block submit — the daemon's
 *  POST /api/sources validation is the authority. */
export async function detectVault(path: string): Promise<VaultDetection> {
  let rootEntries: Awaited<ReturnType<typeof readDir>>;
  try {
    rootEntries = await readDir(path);
  } catch {
    return {
      isVault: false,
      isLogseqGraph: false,
      sourceType: "directory",
      docCount: 0,
      countCapped: false,
      hasValidDoc: false,
      unreadable: true,
    };
  }

  const isVault = rootEntries.some((e) => e.name === ".obsidian" && e.isDirectory);
  const isLogseqGraph = rootEntries.some((e) => e.name === "logseq" && e.isDirectory) &&
    rootEntries.some((e) => LOGSEQ_PAGE_DIRS.includes(e.name ?? "") && e.isDirectory);
  const sourceType = isVault ? ("obsidian" as const) : isLogseqGraph ? ("logseq" as const) : ("directory" as const);
  const extensions = sourceType === "directory" ? DIRECTORY_EXTENSIONS : OBSIDIAN_EXTENSIONS;

  let docCount = 0;
  let entriesVisited = 0;
  let capped = false;

  const queue: Array<{ dir: string; entries: typeof rootEntries; depth: number }> = [
    { dir: path, entries: rootEntries, depth: 1 },
  ];

  while (queue.length > 0 && !capped) {
    const { dir, entries, depth } = queue.shift()!;
    for (const entry of entries) {
      if (entriesVisited >= MAX_ENTRIES) {
        capped = true;
        break;
      }
      entriesVisited += 1;
      const name = entry.name ?? "";
      if (name.startsWith(".")) continue; // dot files and dot dirs (.obsidian, .git…)
      // A graph's logseq/ folder holds backup copies of its pages.
      if (sourceType === "logseq" && depth === 1 && !LOGSEQ_PAGE_DIRS.includes(name)) continue;
      if (entry.isDirectory) {
        if (depth < MAX_DEPTH) {
          try {
            const children = await readDir(`${dir}/${name}`);
            queue.push({ dir: `${dir}/${name}`, entries: children, depth: depth + 1 });
          } catch {
            // unreadable subdir: skip, keep walking
          }
        }
      } else if (extensions.some((ext) => name.toLowerCase().endsWith(ext))) {
        docCount += 1;
      }
    }
  }

  return {
    isVault,
    isLogseqGraph,
    sourceType,
    docCount,
    countCapped: capped,
    hasValidDoc: docCount > 0,
    unreadable: false,
  };
}
