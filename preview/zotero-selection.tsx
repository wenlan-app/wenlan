// SPDX-License-Identifier: AGPL-3.0-only
// Fixture only: Tauri responses are supplied by the browser test.
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { initializeI18n } from "../src/i18n";
import ZoteroSourceDialog from "../src/components/memory/sources/ZoteroSourceDialog";
import "../src/index.css";
const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
void initializeI18n(undefined, { systemLanguages: ["en"] }).then(() => {
  createRoot(document.getElementById("root")!).render(<QueryClientProvider client={client}><ZoteroSourceDialog fixedSpace="Research" onClose={() => {}} /></QueryClientProvider>);
});
