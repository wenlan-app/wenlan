// SPDX-License-Identifier: AGPL-3.0-only
import type { AppLocale } from "../../src/i18n/locales";
import type { AgentConnection, MemoryItem, PageChange, PageReviewAvailability, PageReviewOutcome } from "../../src/lib/tauri";
import type { SpacesNavigationFixture } from "../fixtures/spacesNavigation";
import type { TauriMockBasisScenario } from "./pageBasis";
import type { TauriMockCodeBasisScenario } from "./pageCodeBasis";

export type { TauriMockBasisScenario } from "./pageBasis";
export type { TauriMockCodeBasisScenario } from "./pageCodeBasis";

export type MockCommandCall = {
  readonly command: string;
  readonly args: unknown;
};

export type MockFailure = {
  readonly command: string;
  readonly message: string;
  readonly times?: number;
};

export type TauriMockPageScenario = {
  readonly projectionPendingOnce?: boolean;
  readonly recentChanges?: readonly PageChange[];
  readonly outboundLinks?: Readonly<Record<string, readonly { label: string; target_page_id: string | null }[]>>;
  readonly daemonVersion?: string;
  readonly saveDaemonVersion?: string;
  /**
   * Page-review readiness and refusals. `supported` drives the daemon-cutover
   * gate; `outcome` forces one answer for the refusals a fixture cannot
   * provoke on its own (a capability that does not verify, a missing install
   * secret). The stale path needs neither — the runtime reaches it by
   * comparing content, as the daemon does.
   */
  readonly pageReview?: {
    readonly availability?: PageReviewAvailability;
    readonly outcome?: PageReviewOutcome;
  };
  /** Notes that rely on other notes; see `pageBasis.ts`. */
  readonly basis?: TauriMockBasisScenario;
  /** Notes that rely on lines of code; see `pageCodeBasis.ts`. */
  readonly codeBasis?: TauriMockCodeBasisScenario;
  readonly firstWriteRemoteMutation?: {
    readonly pageId: string;
    readonly content: string;
  };
};

export type InstallTauriMockOptions = {
  /** Opt-in for preference persistence journeys across a real browser reload. */
  readonly preserveLocalStorage?: boolean;
  /**
   * The Wiki home hides its directory while the getting-started checklist is
   * open, and most fixtures are small libraries with no connected tool, so the
   * checklist counts as dismissed by default. Pass true in the few specs that
   * exercise the checklist itself.
   */
  readonly gettingStarted?: boolean;
  readonly locale: AppLocale;
  readonly rawActions: readonly string[];
  /** Override registered tools for journeys whose real entry point depends on connection state. */
  readonly agents?: readonly AgentConnection[];
  readonly memories?: readonly MemoryItem[];
  readonly fixture?: SpacesNavigationFixture;
  readonly failures?: readonly MockFailure[];
  /**
   * Milliseconds to hold a command before answering, keyed by command name.
   * A real library answers `get_page_map` in one to two seconds, and several
   * canvas defects only exist inside that window.
   */
  readonly delays?: Readonly<Record<string, number>>;
  readonly localStorage?: Readonly<Record<string, string>>;
  readonly pageScenario?: TauriMockPageScenario;
};

export type BrowserErrorCapture = {
  readonly pageErrors: string[];
  readonly consoleErrors: string[];
};
