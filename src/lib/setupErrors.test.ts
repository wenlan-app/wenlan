// SPDX-License-Identifier: AGPL-3.0-only
/**
 * One case per error class, plus the two rules that matter more than the
 * classification itself: the raw text always survives as a detail line, and
 * an unrecognised failure still gets a heading instead of a bare string.
 */
import { describe, expect, it } from "vitest";

import {
  describeSetupError,
  setupErrorHeadingKey,
  type SetupErrorKind,
} from "./setupErrors";

describe("describeSetupError", () => {
  it.each([
    ["connection refused", "connection"],
    ["HTTP GET /api/health: connection refused (os error 61)", "connection"],
    ["The Wenlan daemon isn't reachable.", "connection"],
    ["error sending request for url (http://127.0.0.1:7878): tcp connect error", "connection"],
    ["HTTP POST /api/ingest/memory: error sending request for url (http://127.0.0.1:7878/api/ingest/memory)", "connection"],
    ["operation timed out", "timeout"],
    ["error sending request: operation timed out (ETIMEDOUT)", "timeout"],
    ["deadline has elapsed", "timeout"],
    ["Permission denied (os error 13)", "permission"],
    ["EACCES: permission denied, open '/Users/x/.codex/config.toml'", "permission"],
    ["failed to write config: Operation not permitted", "permission"],
    ["No space left on device (os error 28)", "disk"],
    ["ENOSPC: no space left on device", "disk"],
    ["Read-only file system (os error 30)", "disk"],
    ["the model registry returned 500", "unknown"],
    ["", "unknown"],
  ] as [string, SetupErrorKind][])(
    "classifies %j as %s",
    (raw, kind) => {
      expect(describeSetupError(raw).kind).toBe(kind);
    },
  );

  it("prefers timeout over connection when a connection timed out", () => {
    // "Connection timed out" matches both vocabularies. Timeout is the more
    // actionable of the two: the socket was reachable, the answer was not.
    expect(describeSetupError("Connection timed out").kind).toBe("timeout");
  });

  it("keeps the raw text as the detail line, never swallowing it", () => {
    const described = describeSetupError(
      new Error("HTTP PUT /api/config: connection refused"),
    );
    expect(described.kind).toBe("connection");
    expect(described.detail).toBe("HTTP PUT /api/config: connection refused");
  });

  it("unwraps strings, Errors and message-carrying objects, and drops the rest", () => {
    expect(describeSetupError("plain string").detail).toBe("plain string");
    expect(describeSetupError(new Error("from an Error")).detail).toBe("from an Error");
    expect(describeSetupError({ message: "from an object" }).detail).toBe("from an object");
    // No "[object Object]" on screen: an unreadable failure degrades to a
    // bare localized heading with no detail line at all.
    expect(describeSetupError({ code: 500 }).detail).toBe("");
    expect(describeSetupError(null).detail).toBe("");
    expect(describeSetupError(undefined).detail).toBe("");
  });

  it("strips the Error: prefix a stringified rejection carries", () => {
    expect(describeSetupError("Error: connection refused").detail).toBe(
      "connection refused",
    );
  });

  it("maps every kind to a setup.errors heading key", () => {
    const kinds: SetupErrorKind[] = [
      "connection",
      "timeout",
      "permission",
      "disk",
      "unknown",
    ];
    for (const kind of kinds) {
      expect(setupErrorHeadingKey(kind)).toBe(`setup.errors.${kind}`);
    }
  });
});
