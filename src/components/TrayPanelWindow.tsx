// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { initializeI18n } from "../i18n";
import { applyTheme } from "../lib/theme";
import TrayPanel, { TRAY_PANEL_OPENED_EVENT } from "./TrayPanel";

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

export default function TrayPanelWindow() {
  // The webview lives on while hidden, and the user may have changed the
  // theme or the language in the main window since it last opened.
  useEffect(() => {
    const unlisten = listen(TRAY_PANEL_OPENED_EVENT, () => {
      applyTheme();
      void initializeI18n();
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  return (
    <QueryClientProvider client={queryClient}>
      <TrayPanel />
    </QueryClientProvider>
  );
}
