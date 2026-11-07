// SPDX-License-Identifier: AGPL-3.0-only
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { initializeI18n } from "../src/i18n";
import SourcesSection from "../src/components/memory/sources/SourcesSection";
import "../src/index.css";
void initializeI18n(undefined,{systemLanguages:["en"]}).then(() => createRoot(document.getElementById("root")!).render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><main style={{maxWidth:700,margin:"4rem auto"}}><SourcesSection /></main></QueryClientProvider>));
