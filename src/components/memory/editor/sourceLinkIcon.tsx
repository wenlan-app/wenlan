// SPDX-License-Identifier: AGPL-3.0-only
import { createRoot } from "react-dom/client";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import { getSourceAppFromUrl } from "../sources/sourcePresentation";
import type { SourceIconMount } from "./sourcePaste";

/** The editor's site icons are the same ones Sources shows. */
export const mountSourceLinkIcon: SourceIconMount = (container, url) => {
  const root = createRoot(container);
  root.render(<SourceTypeIcon kind={getSourceAppFromUrl(url) ?? "webpage"} size={14} />);
  // CodeMirror can drop a widget while React is rendering; unmount after it.
  return () => queueMicrotask(() => root.unmount());
};
