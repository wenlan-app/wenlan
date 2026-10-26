// SPDX-License-Identifier: AGPL-3.0-only
// Actual Main/FirstUseGuide components with the existing isolated Tauri test runtime.
import React from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import Main from "../src/components/memory/Main";
import { initializeI18n } from "../src/i18n";
import "../src/index.css";
const params = new URLSearchParams(location.search);
const locale = params.get("locale") ?? "zh-Hant";
const theme = params.get("theme") === "dark" ? "dark" : "light";
document.documentElement.dataset.theme = theme;
document.documentElement.lang = locale;
const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
void initializeI18n(undefined, { storage: { getItem: () => locale, setItem: () => {} }, systemLanguages: [locale] }).then(() => {
  // `view=import` opens straight on the Import page, for the import finish states.
  const initialView = params.get("view") === "import" ? { kind: "import" as const } : undefined;
  createRoot(document.getElementById("root")!).render(<React.StrictMode><QueryClientProvider client={client}><Main initialView={initialView} /></QueryClientProvider></React.StrictMode>);
});
