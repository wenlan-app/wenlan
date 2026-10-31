// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Turns a setup failure into a localized heading plus the raw text.
 *
 * Every row of the wizard's Setting-up step used to render a bare
 * `String(err)`: a first-run user read "HTTP GET /api/health: connection
 * refused (os error 61)" and had no idea whether that was their fault, their
 * disk, or a bug. The raw text is still the only thing a bug report can use,
 * so nothing here replaces it. It gets a sentence above it instead.
 *
 * The classification is deliberately shallow — five buckets over the failure
 * vocabularies the daemon, reqwest, Node and the OS actually emit. A miss
 * costs the user nothing: `unknown` still carries a heading and the raw line.
 */

/** Coarse class of a setup failure. `unknown` is a real answer, not a gap. */
export type SetupErrorKind =
  | "connection"
  | "timeout"
  | "permission"
  | "disk"
  | "unknown";

/** A failure the UI can render: a class to head it, the raw text to prove it. */
export interface DescribedSetupError {
  kind: SetupErrorKind;
  /** The raw failure text, verbatim, or "" when there was nothing readable. */
  detail: string;
}

/**
 * Pull the readable text out of whatever a rejection carried. Strings and
 * Errors pass through; anything else degrades to "" so a row shows a bare
 * heading instead of "[object Object]". Mirrors `formatImportErrorDetail`
 * in `src/components/memory/importCopy.ts`, which does the same job for the
 * import surfaces.
 */
export function rawSetupErrorText(err: unknown): string {
  let text: string;
  if (typeof err === "string") text = err;
  else if (err instanceof Error) text = err.message;
  else if (
    err !== null &&
    typeof err === "object" &&
    "message" in err &&
    typeof (err as { message: unknown }).message === "string"
  ) {
    text = (err as { message: string }).message;
  } else if (typeof err === "number" || typeof err === "boolean") {
    text = String(err);
  } else {
    text = "";
  }
  // A rejection that already went through String() carries the constructor
  // name. It is noise in a detail line, and the heading above says the same
  // thing better.
  return text.replace(/^Error:\s*/, "").trim();
}

// Order is load-bearing where the vocabularies overlap. "Connection timed
// out" is both; timeout is the more actionable of the two, because the
// socket was reachable and the answer was not.
const PATTERNS: [SetupErrorKind, RegExp][] = [
  [
    "timeout",
    /\btimed?\s?out\b|\btimeout\b|\betimedout\b|deadline (has )?(elapsed|exceeded)/i,
  ],
  [
    "permission",
    /permission denied|\beacces\b|\beperm\b|operation not permitted|access is denied|not authorized|unauthorized|\bforbidden\b/i,
  ],
  [
    "disk",
    /no space left|\benospc\b|disk (is )?full|quota exceeded|insufficient (disk )?space|read-only file system|\berofs\b/i,
  ],
  [
    "connection",
    /connection refused|\beconnrefused\b|\beconnreset\b|connection reset|error sending request|unreachable|isn't reachable|is not reachable|could not connect|couldn't connect|failed to connect|connect error|no route to host|\benotfound\b|network error/i,
  ],
];

/**
 * Classify a setup failure and keep its raw text.
 *
 * Callers render `t(setupErrorHeadingKey(kind))` as the heading and `detail`
 * as a smaller line under it, skipping the second line when detail is "".
 */
export function describeSetupError(err: unknown): DescribedSetupError {
  const detail = rawSetupErrorText(err);
  for (const [kind, pattern] of PATTERNS) {
    if (pattern.test(detail)) return { kind, detail };
  }
  return { kind: "unknown", detail };
}

/**
 * True when a request failed because the daemon did not answer at all
 * (refused, reset, never sent), as opposed to answering with an error.
 * A timeout is not this: the socket was reachable.
 */
export function isDaemonUnreachableError(err: unknown): boolean {
  return describeSetupError(err).kind === "connection";
}

/** i18n key for a class's heading. One place, so a new class cannot ship
 *  with no sentence attached to it. */
export function setupErrorHeadingKey(
  kind: SetupErrorKind,
): `setup.errors.${SetupErrorKind}` {
  return `setup.errors.${kind}`;
}
