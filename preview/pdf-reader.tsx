// SPDX-License-Identifier: AGPL-3.0-only
// A real renderer fixture: Playwright supplies the repository's public paper.
import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import "pdfjs-dist/legacy/build/pdf.mjs";
import { PDFViewer } from "pdfjs-dist/legacy/web/pdf_viewer.mjs";
import { initializeI18n } from "../src/i18n";
import PdfReader from "../src/components/memory/sources/PdfReader";

// Expose the actual PDF.js viewer only in this isolated test fixture, so a
// test can exercise its rotation/reset lifecycle even though the product has
// no rotation control.
const setDocument = PDFViewer.prototype.setDocument;
PDFViewer.prototype.setDocument = function (doc) {
  if (doc) (window as any).__pdfFixtureViewer = this;
  return setDocument.call(this, doc);
};
const options = new URLSearchParams(location.search);
const annotation = { key: "ANNOT001", attachment_key: "PAPER001", page_index: 0,
  rects: [[70, 690, 300, 705] as [number, number, number, number]], color: "#ffdf00", text: "fixture", comment: "" };
function Preview() {
  const [bytes, setBytes] = useState<Uint8Array | null>(null);
  useEffect(() => { void fetch("/pdf-fixture.pdf").then(r => r.arrayBuffer()).then(b => setBytes(new Uint8Array(b))); }, []);
  const target = options.has("page") ? Number(options.get("page")) : undefined;
  return <main style={{maxWidth: 680, margin: "24px auto", fontFamily: "system-ui"}}>
    {bytes && <PdfReader data={bytes} documentId="public-bert-fixture" page={target}
      quote={options.get("quote") ?? undefined} annotations={options.has("annotation") ? [annotation] : []} />}
  </main>;
}
void initializeI18n(undefined, { systemLanguages: ["en"] }).then(() => {
  createRoot(document.getElementById("root")!).render(<Preview />);
});
