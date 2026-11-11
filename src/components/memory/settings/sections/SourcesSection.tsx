// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import SourcesList from "../../sources/SourcesSection";
import { Button, Card, SectionHeader } from "../primitives";

// Past AI chats and everything else come in through the one "Add your
// material" (its "Add source…" opens it), not separate import blocks here.
// The quick paste of what an AI remembers is the link under past AI chats.
export default function SourcesSection({ onImport, onOpenLibrary }: { onImport?: () => void; onOpenLibrary?: () => void }) {
  const { t } = useTranslation();
  return (
    <>
      {/* ── Sources ────────────────────────────────────────────── */}
      <section className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SectionHeader label={t("settings.sources.connectedTitle")} />
        <SourcesList onQuickChats={onImport} />
      </section>

      {/* What was added has no place in the navigation: a source is what a
          sentence cites, read from the page that cites it. The full list of
          added files and links stays one step away, here. */}
      {onOpenLibrary && <section className="mem-fade-up" style={{ animationDelay: "15ms" }}>
        <SectionHeader label={t("settings.sources.libraryTitle")} />
        <Card padding="rows">
          <div className="px-5 py-4">
            <div className="settings-import-row flex items-center justify-between gap-4">
              <div className="min-w-0">
                <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)", marginTop: "2px", lineHeight: "1.5" }}>
                  {t("settings.sources.libraryDescription")}
                </p>
              </div>
              <Button variant="secondary" size="sm" className="shrink-0" onClick={onOpenLibrary}>
                {t("settings.sources.openLibrary")}
              </Button>
            </div>
          </div>
        </Card>
      </section>}

    </>
  );
}
