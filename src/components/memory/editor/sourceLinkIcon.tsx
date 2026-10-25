// SPDX-License-Identifier: AGPL-3.0-only
import { createRoot } from "react-dom/client";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import { getSourceAppFromUrl } from "../sources/sourcePresentation";
import type { SourceIconMount } from "./sourcePaste";

export interface SourceLinkIconActions {
  /** Says what the icon means, such as "Saved in Sources". */
  label: string;
  open(url: string): void;
}

/** The editor's site icons are the same ones Sources shows; a click opens the source. */
export function sourceLinkIconMount(actions?: () => SourceLinkIconActions): SourceIconMount {
  return (container, url) => {
    const root = createRoot(container);
    const icon = <SourceTypeIcon kind={getSourceAppFromUrl(url) ?? "webpage"} size={14} />;
    const current = actions?.();
    root.render(current ? (
      <button
        type="button"
        title={current.label}
        aria-label={current.label}
        // Keep the caret where it is: the click is not a move into the link.
        onMouseDown={(event) => event.preventDefault()}
        onClick={(event) => {
          event.preventDefault();
          actions?.().open(url);
        }}
      >
        {icon}
      </button>
    ) : <span aria-hidden="true">{icon}</span>);
    // CodeMirror can drop a widget while React is rendering; unmount after it.
    return () => queueMicrotask(() => root.unmount());
  };
}

export const mountSourceLinkIcon: SourceIconMount = sourceLinkIconMount();
