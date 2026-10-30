// The Windows release payload, read out of `scripts/release_targets.py`.
//
// That file is the one place the shipped Windows ZIP inventory is written down.
// The Tauri bundle, the release workflows, the first-run ZIP check,
// prepare-sidecars.sh and the dev daemon stage all repeat parts of it, and the
// tests that hold them together compare against this list instead of carrying
// a third copy. The file is read as text so the tests do not need a Python
// interpreter on the machine that runs vitest.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const WINDOWS_ZIP = "wenlan-windows-x64.zip";

/** Every member of the Windows release ZIP, in the order release_targets.py lists them. */
export function windowsReleaseMembers(repoRoot: string): string[] {
  const source = readFileSync(resolve(repoRoot, "scripts/release_targets.py"), "utf8");
  const asset = source.indexOf(`"name": "${WINDOWS_ZIP}"`);
  if (asset < 0) throw new Error(`release_targets.py no longer lists ${WINDOWS_ZIP}`);
  const members = source.slice(asset).match(/"members":\s*\[([^\]]*)\]/);
  if (!members) throw new Error(`release_targets.py lists ${WINDOWS_ZIP} without members`);
  const names = [...members[1]!.matchAll(/"([^"]+)"/g)].map((match) => match[1]!);
  if (names.length === 0) throw new Error(`release_targets.py lists ${WINDOWS_ZIP} with no members`);
  return names;
}

/** The files that travel beside the executables: every member that is not an `.exe`. */
export function windowsRuntimeFiles(repoRoot: string): string[] {
  return windowsReleaseMembers(repoRoot).filter((name) => !name.toLowerCase().endsWith(".exe"));
}

/** The libraries the executables resolve from their own directory. */
export function windowsRuntimeLibraries(repoRoot: string): string[] {
  return windowsRuntimeFiles(repoRoot).filter((name) => name.toLowerCase().endsWith(".dll"));
}
