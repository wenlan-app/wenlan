import type { WikiInventoryScope, PageProjectionIssue } from "../pages/pageInventory";
import type { IndexedFileInfo } from "../../../lib/tauri";
import type { SettingsSection } from "../settings/SettingsSidebar";

export type View =
  | { readonly kind: "activity" }
  | { readonly kind: "connect-agent" }
  | { readonly kind: "distill-review"; readonly reviewItemId?: string }
  | { readonly kind: "entities" }
  | { readonly kind: "entity"; readonly entityId: string }
  | { readonly kind: "graph"; readonly focusPageId?: string; readonly focusThingId?: string; /** Opens the map on one Space with its regions drawn (the Space page's "Show on the big map"). */ readonly focusSpace?: string; /** Set by home's "Replay": the press it answers, so going back to this map never plays again (UX-GRAPH-6). */ readonly replay?: number }
  | { readonly kind: "home" }
  | { readonly kind: "first-use"; readonly showKnowledge?: boolean; readonly batchId?: string }
  | { readonly kind: "import"; readonly fromFirstUse?: boolean; readonly fromHome?: boolean }
  | { readonly kind: "memory"; readonly sourceId: string }
  | { readonly kind: "page"; readonly pageId: string; readonly mode?: "read" | "edit"; readonly inventoryScope?: WikiInventoryScope; readonly projectionIssue?: PageProjectionIssue }
  | {
    readonly kind: "page-draft";
    readonly draftId?: string;
    readonly folderPath?: string;
    readonly inventoryScope?: WikiInventoryScope;
    readonly space: string | null;
    /** Stable across autosaves and history; distinct for each new draft intent. */
    readonly sessionKey?: number;
  }
  | { readonly kind: "pages"; readonly inventoryScope?: WikiInventoryScope }
  | { readonly kind: "recaps" }
  | { readonly kind: "settings"; readonly section?: SettingsSection }
  | { readonly kind: "sources" }
  | { readonly kind: "source-document"; readonly file: IndexedFileInfo }
  | { readonly kind: "space"; readonly spaceId: string | null; readonly spaceName: string }
  | { readonly kind: "spaces"; readonly create?: boolean }
  | { readonly kind: "stream" }
  | { readonly kind: "thing"; readonly thingId: string };

export type GlobalNavigation = "home" | "memories" | "pages" | "entities" | "spaces" | "graph" | "sources";

function assertNever(value: never): never {
  throw new TypeError(`Unsupported view: ${String(value)}`);
}

export function activeNavigationForView(view: View): GlobalNavigation | null {
  switch (view.kind) {
    case "home":
    case "first-use":
    case "thing":
      return "home";
    case "activity":
      return null;
    case "memory":
    case "recaps":
    case "stream":
      return "memories";
    case "distill-review":
    case "page":
    case "page-draft":
    case "pages":
      return "pages";
    // The topic list opens from the graph, so the graph stays lit there.
    case "entities":
      return "graph";
    case "entity":
      return "entities";
    case "space":
    case "spaces":
      return "spaces";
    case "graph":
      return "graph";
    case "import":
    case "sources":
    case "source-document":
      return "sources";
    case "connect-agent":
    case "settings":
      return null;
    default:
      return assertNever(view);
  }
}
