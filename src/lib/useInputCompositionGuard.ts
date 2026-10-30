// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { observeInputComposition } from "./inputComposition";
import { setEditorComposing } from "./tauri";

// Keep native notifications ordered across rapid composition cycles and remounts.
let pending = Promise.resolve();
export function useInputCompositionGuard(): void {
  useEffect(() => observeInputComposition(active => {
    pending = pending
      .then(() => setEditorComposing(active))
      .catch(error => { console.warn("Failed to synchronize input composition", error); });
  }), []);
}
