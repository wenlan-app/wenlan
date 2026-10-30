// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { requestFocusedMarkdownLink } from "../lib/focusedEditorCommand";
import { useInputCompositionGuard } from "../lib/useInputCompositionGuard";
import { applyTheme } from "../lib/theme";
import QuickCapture from "./QuickCapture";

const queryClient = new QueryClient();

export default function QuickCaptureWindow() {
  useInputCompositionGuard();
  useEffect(() => {
    const editorUnlisten = listen("editor-primary-shortcut", () => { requestFocusedMarkdownLink(); });
    const win = getCurrentWindow();
    const unlisten = win.onFocusChanged(({ payload: focused }) => {
      if (focused) {
        // Re-apply theme every time window becomes visible. The quick capture
        // webview is long-lived but hidden between uses; the user may have
        // changed the theme in the main window while it was hidden.
        applyTheme();
        invoke("position_quick_capture");
      }
    });
    return () => {
      editorUnlisten.then(fn => fn());
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <QueryClientProvider client={queryClient}>
      <QuickCapture
        isOpen={true}
        onClose={() => invoke("dismiss_quick_capture")}
        standalone
      />
    </QueryClientProvider>
  );
}
