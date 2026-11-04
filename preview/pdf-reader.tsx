// SPDX-License-Identifier: AGPL-3.0-only
// A real renderer fixture: Playwright supplies the repository's public paper.
import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { initializeI18n } from "../src/i18n";
import PdfReader from "../src/components/memory/sources/PdfReader";
function Preview() {
  const [bytes, setBytes] = useState<Uint8Array | null>(null);
  useEffect(() => { void fetch("/pdf-fixture.pdf").then(r => r.arrayBuffer()).then(b => setBytes(new Uint8Array(b))); }, []);
  return <main style={{maxWidth: 680, margin: "24px auto", fontFamily: "system-ui"}}>
    {bytes && <PdfReader data={bytes} documentId="public-bert-fixture" />}
  </main>;
}
void initializeI18n(undefined, { systemLanguages: ["en"] }).then(() => {
  createRoot(document.getElementById("root")!).render(<Preview />);
});
