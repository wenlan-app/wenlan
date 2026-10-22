// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowCounterClockwise,
  ArrowLeft,
  ArrowRight,
  Brain,
  Check,
  Copy,
  FileText,
  Pause,
  Play,
  SkipForward,
  X,
} from "@phosphor-icons/react";
import { clipboardWrite } from "../../lib/tauri";
import {
  getSampleData,
  resolveSampleLocale,
  type SampleCitation,
  type SampleClient,
  type SamplePageParagraph,
  type SampleSource,
  type SampleSourceType,
} from "./firstUseSampleData";
import "./firstUseGuide.css";

export interface FirstUseSampleProps {
  /** Header back button: returns to wherever the walkthrough was opened from. */
  onBack: () => void;
  backLabel?: string;
  /** Closing action of the "Let AI use your knowledge" panel. */
  onFinish: () => void;
  finishLabel?: string;
}

type SamplePhase = 0 | 1 | 2 | 3;

const FINAL_PHASE: SamplePhase = 3;
/** Demo-only autoplay cadence. The live view never uses timers. */
const SAMPLE_PHASE_MS = 3000;

type CopyKey = "recall" | "handoff" | "brief";

type SampleDialogState = { kind: "source"; sourceId: string; citationId: string | null };

const FOCUSABLE_SELECTOR =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

function SourceGlyph({ type }: { type: SampleSourceType }) {
  if (type === "memory") return <Brain aria-hidden="true" weight="regular" />;
  return <FileText aria-hidden="true" weight="regular" />;
}

function HighlightedLine({ line, quote }: { line: string; quote: string }) {
  const index = quote ? line.indexOf(quote) : -1;
  if (index < 0) return <>{line}</>;
  return (
    <>
      {line.slice(0, index)}
      <mark>{quote}</mark>
      {line.slice(index + quote.length)}
    </>
  );
}

/**
 * Accessible modal: focuses the dialog on open, traps Tab/Shift+Tab inside,
 * closes on Escape, restores the trigger's focus on close. The caller renders
 * the background with `inert` while a dialog is open.
 */
function SampleDialog({
  labelId,
  onClose,
  testId,
  sourceType,
  restoreFocusTo,
  children,
}: {
  labelId: string;
  onClose: () => void;
  testId: string;
  sourceType?: SampleSourceType;
  restoreFocusTo: HTMLElement | null;
  children: ReactNode;
}) {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const restoreRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    restoreRef.current = restoreFocusTo ??
      (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const node = dialogRef.current;
    node?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab" || !node) return;
      const items = Array.from(
        node.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
      );
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === node || !node.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === node || !node.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      restoreRef.current?.focus();
    };
  }, [onClose, restoreFocusTo]);

  return (
    <div
      className="fus-overlay"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelId}
        tabIndex={-1}
        data-testid={testId}
        data-source-type={sourceType}
        className="fus-dialog"
      >
        {children}
      </div>
    </div>
  );
}

export function FirstUseSample({
  onBack,
  backLabel,
  onFinish,
  finishLabel,
}: FirstUseSampleProps) {
  const { t, i18n } = useTranslation();
  const data = getSampleData(resolveSampleLocale(i18n.language));
  const { sources, citations, page, ai } = data;

  const [phase, setPhase] = useState<SamplePhase>(0);
  const [playing, setPlaying] = useState<boolean>(() => !prefersReducedMotion());
  const [reducedMotion, setReducedMotion] = useState<boolean>(() => prefersReducedMotion());
  const [dialog, setDialog] = useState<SampleDialogState | null>(null);
  const dialogTrigger = useRef<HTMLElement | null>(null);
  const [aiOpen, setAiOpen] = useState(false);
  // The sample chat is in Codex, so its commands open first.
  const [client, setClient] = useState<SampleClient>("codex");
  const [copiedKey, setCopiedKey] = useState<CopyKey | null>(null);
  const [copyFailedKey, setCopyFailedKey] = useState<CopyKey | null>(null);

  // Stale-copy guard: only the latest copy request may report success or
  // failure. Client switches, replay, and unmount invalidate pending ones.
  const copySeq = useRef(0);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      copySeq.current += 1;
    };
  }, []);

  // Demo-only autoplay: ~3 s per phase, cleaned up on every change/unmount.
  // Reduced-motion users get a fully manual walkthrough instead.
  useEffect(() => {
    if (!playing || reducedMotion || phase >= FINAL_PHASE) return;
    const timer = window.setTimeout(() => {
      const next = (phase + 1) as SamplePhase;
      setPhase(next);
      if (next >= FINAL_PHASE) setPlaying(false);
    }, SAMPLE_PHASE_MS);
    return () => window.clearTimeout(timer);
  }, [playing, phase, reducedMotion]);

  // Follow the OS motion preference live; dropping into reduced motion stops
  // playback and removes the nonfunctional Play control.
  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => {
      const reduced = query.matches;
      setReducedMotion(reduced);
      if (reduced) setPlaying(false);
    };
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  const selectPhase = (next: SamplePhase) => {
    setPhase(next);
    setPlaying(false);
  };

  const replay = () => {
    copySeq.current += 1;
    setCopiedKey(null);
    setCopyFailedKey(null);
    setDialog(null);
    setAiOpen(false);
    setPhase(0);
    setPlaying(!prefersReducedMotion());
  };

  const closeDialog = useCallback(() => setDialog(null), []);

  const copyCommand = async (key: CopyKey, value: string) => {
    const seq = copySeq.current + 1;
    copySeq.current = seq;
    setCopiedKey(null);
    setCopyFailedKey(null);
    try {
      await clipboardWrite(value);
    } catch {
      // A clipboard rejection is reported as a failure, never as success —
      // and only when this request is still the latest one.
      if (!mountedRef.current || copySeq.current !== seq) return;
      setCopyFailedKey(key);
      return;
    }
    if (!mountedRef.current || copySeq.current !== seq) return;
    setCopiedKey(key);
  };

  const switchClient = (next: SampleClient) => {
    copySeq.current += 1;
    setClient(next);
    setCopiedKey(null);
    setCopyFailedKey(null);
  };

  const citationById = (id: string): SampleCitation | undefined =>
    citations.find((entry) => entry.id === id);

  const sourceById = (id: string): SampleSource | undefined =>
    sources.find((source) => source.id === id);

  const quotesFor = (source: SampleSource): string[] =>
    // Two citations can quote the same line of one source; the rack shows it once.
    [...new Set(citations.filter((entry) => entry.sourceId === source.id).map((entry) => entry.quote))];

  const paragraphSources = (paragraph: SamplePageParagraph): SampleSource[] => {
    const seen = new Set<string>();
    const result: SampleSource[] = [];
    for (const citationId of paragraph.citationIds) {
      const entry = citationById(citationId);
      const source = entry ? sourceById(entry.sourceId) : undefined;
      if (source && !seen.has(source.id)) {
        seen.add(source.id);
        result.push(source);
      }
    }
    return result;
  };

  const openSource = (sourceId: string, citationId: string | null) =>
    setDialog({ kind: "source", sourceId, citationId });

  const dialogSource = dialog?.kind === "source" ? sourceById(dialog.sourceId) : undefined;
  const dialogCitation =
    dialog?.kind === "source" && dialog.citationId ? citationById(dialog.citationId) : undefined;

  const commands = data.commands[client];
  const handoffCommand = commands.handoff;
  const clientTabs: Array<{ id: SampleClient; label: string }> = [
    { id: "chatgpt", label: t("firstUse.sample.clientChatgpt") },
    { id: "codex", label: t("firstUse.sample.clientCodex") },
    { id: "claude", label: t("firstUse.sample.clientClaude") },
  ];

  // The page leads; the next two steps show where its lines came from (a chat
  // decision, then a file), and the last shows an AI answering from it.
  const phaseNames = [
    t("firstUse.sample.phasePage"),
    t("firstUse.sample.phaseMemory"),
    t("firstUse.sample.phaseFile"),
    t("firstUse.sample.phaseAi"),
  ];
  const phaseBodies = [
    t("firstUse.sample.phasePageBody"),
    t("firstUse.sample.phaseMemoryBody"),
    t("firstUse.sample.phaseFileBody"),
    t("firstUse.sample.phaseAiBody"),
  ];
  const focusType: SampleSourceType | null =
    phase === 1 ? "memory" : phase === 2 ? "file" : null;
  const showPlay = !reducedMotion && !playing && phase < FINAL_PHASE;

  return (
    <>
      <div
        data-testid="first-use-sample"
        className="fug-sample"
        inert={dialog ? true : undefined}
        onClickCapture={(event) => {
          // macOS WebKit does not necessarily focus a clicked button.
          const trigger = event.target instanceof Element
            ? event.target.closest('button[aria-haspopup="dialog"]') : null;
          if (trigger instanceof HTMLElement) dialogTrigger.current = trigger;
        }}
      >
        {/* The steps and the way out share one row, so the walkthrough starts at
            the top of the window and its last steps fit without scrolling. */}
        <div className="fus-top">
        <ol className="fus-phases" aria-label={t("firstUse.sample.animationLabel")}>
          {phaseNames.map((name, index) => {
            const id = index as SamplePhase;
            return (
              <li key={name}>
                <button
                  type="button"
                  data-testid={`sample-phase-${index}`}
                  data-active={phase === id}
                  data-complete={phase > id}
                  className={phase === id ? "fus-phase is-active" : "fus-phase"}
                  aria-current={phase === id ? "step" : undefined}
                  onClick={() => selectPhase(id)}
                >
                  <span className="fus-phase-number" aria-hidden="true">
                    {phase > id ? <Check weight="bold" /> : index + 1}
                  </span>
                  {name}
                </button>
              </li>
            );
          })}
        </ol>
          <button type="button" className="fug-back fus-exit" onClick={onBack}>
            <ArrowLeft aria-hidden="true" weight="regular" />
            {backLabel ?? t("firstUse.sample.back")}
          </button>
        </div>

        <div className="fus-step-row">
        <p className="fus-phase-body">{phaseBodies[phase]}</p>
        <div className="fus-controls">
          {playing ? (
            <button
              type="button"
              className="fug-button-secondary"
              onClick={() => setPlaying(false)}
              aria-label={t("firstUse.sample.pause")}
            >
              <Pause aria-hidden="true" weight="regular" />
              {t("firstUse.sample.pause")}
            </button>
          ) : null}
          {showPlay ? (
            <button
              type="button"
              className="fug-button-secondary"
              onClick={() => setPlaying(true)}
              aria-label={t("firstUse.sample.play")}
            >
              <Play aria-hidden="true" weight="regular" />
              {t("firstUse.sample.play")}
            </button>
          ) : null}
          <button
            type="button"
            className="fug-button-secondary"
            onClick={replay}
            aria-label={t("firstUse.sample.replay")}
          >
            <ArrowCounterClockwise aria-hidden="true" weight="regular" />
            {t("firstUse.sample.replay")}
          </button>
          {phase < FINAL_PHASE ? (
            <button
              type="button"
              className="fug-button-secondary"
              onClick={() => selectPhase((phase + 1) as SamplePhase)}
              aria-label={t("firstUse.sample.next")}
            >
              {t("firstUse.sample.next")}
              <ArrowRight aria-hidden="true" weight="regular" />
            </button>
          ) : null}
          {phase < FINAL_PHASE ? (
            <button
              type="button"
              className="fug-button-secondary"
              onClick={() => selectPhase(FINAL_PHASE)}
              aria-label={t("firstUse.sample.skipToResult")}
            >
              <SkipForward aria-hidden="true" weight="regular" />
              {t("firstUse.sample.skipToResult")}
            </button>
          ) : null}
        </div>
        </div>

        {aiOpen ? (
          <section className="fus-use" aria-labelledby="fus-use-title">
            <button type="button" className="fug-back" onClick={() => setAiOpen(false)}>
              <ArrowLeft aria-hidden="true" weight="regular" />
              {t("firstUse.sample.backToPage")}
            </button>
            <h2 id="fus-use-title">{t("firstUse.sample.useTitle")}</h2>
            <p>{t("firstUse.sample.useBody")}</p>
            <div className="fus-tabs" role="tablist" aria-label={t("firstUse.sample.useTitle")}>
              {clientTabs.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={client === tab.id}
                  className={client === tab.id ? "fus-tab is-selected" : "fus-tab"}
                  onClick={() => switchClient(tab.id)}
                >
                  {tab.label}
                </button>
              ))}
            </div>
            {client === "chatgpt" ? (
              <p className="fus-note">{t("firstUse.sample.chatgptNote")}</p>
            ) : null}
            <div className="fus-command" role="tabpanel">
              <p className="fus-command-label">{t("firstUse.sample.recallLabel")}</p>
              <div className="fus-command-row">
                <code>{commands.recall}</code>
                <button
                  type="button"
                  className="fus-copy"
                  onClick={() => copyCommand("recall", commands.recall)}
                >
                  {copiedKey === "recall" ? (
                    <Check aria-hidden="true" weight="bold" data-testid="copy-ok" />
                  ) : (
                    <Copy aria-hidden="true" weight="regular" />
                  )}
                  {copiedKey === "recall"
                    ? t("firstUse.sample.copied")
                    : t("firstUse.sample.copyCommand")}
                </button>
              </div>
              {copyFailedKey === "recall" ? (
                <p role="alert" className="fus-copy-error">
                  {t("firstUse.sample.copyFailed")}
                </p>
              ) : null}
            </div>
            <ul className="fus-secondary-commands">
              <li>
                <div>
                  <span>
                    <strong>{t("firstUse.sample.briefLabel")}</strong> {t("firstUse.sample.briefHint")}
                  </span>
                  <code>{commands.brief}</code>
                </div>
                <button
                  type="button"
                  className="fus-icon-button"
                  onClick={() => copyCommand("brief", commands.brief)}
                  aria-label={`${t("firstUse.sample.copyCommand")}: ${t("firstUse.sample.briefLabel")}`}
                >
                  {copiedKey === "brief" ? (
                    <Check aria-hidden="true" weight="bold" data-testid="copy-ok" />
                  ) : (
                    <Copy aria-hidden="true" weight="regular" />
                  )}
                </button>
                <span role="status" className="sr-only">
                  {copiedKey === "brief" ? `${t("firstUse.sample.briefLabel")}: ${t("firstUse.sample.copied")}` : ""}
                </span>
                {copyFailedKey === "brief" ? (
                  <p role="alert" className="fus-copy-error">
                    {t("firstUse.sample.copyFailed")}
                  </p>
                ) : null}
              </li>
              <li>
                {handoffCommand != null ? (
                  <>
                    <div>
                      <span>
                        <strong>{t("firstUse.sample.handoffLabel")}</strong> {t("firstUse.sample.handoffHint")}
                      </span>
                      <code>{handoffCommand}</code>
                    </div>
                    <button
                      type="button"
                      className="fus-icon-button"
                      onClick={() => copyCommand("handoff", handoffCommand)}
                      aria-label={`${t("firstUse.sample.copyCommand")}: ${t("firstUse.sample.handoffLabel")}`}
                    >
                      {copiedKey === "handoff" ? (
                        <Check aria-hidden="true" weight="bold" data-testid="copy-ok" />
                      ) : (
                        <Copy aria-hidden="true" weight="regular" />
                      )}
                    </button>
                    <span role="status" className="sr-only">
                      {copiedKey === "handoff" ? `${t("firstUse.sample.handoffLabel")}: ${t("firstUse.sample.copied")}` : ""}
                    </span>
                    {copyFailedKey === "handoff" ? (
                      <p role="alert" className="fus-copy-error">
                        {t("firstUse.sample.copyFailed")}
                      </p>
                    ) : null}
                  </>
                ) : (
                  <div>
                    <span>
                      <strong>{t("firstUse.sample.handoffLabel")}</strong> {t("firstUse.sample.handoffReadOnlyNote")}
                    </span>
                  </div>
                )}
              </li>
            </ul>
            <p className="fus-note">{t("firstUse.sample.notInLibrary")}</p>
            <div className="fus-cta-row">
              <button type="button" className="fug-button-primary" onClick={onFinish}>
                {finishLabel ?? t("firstUse.sample.finish")}
              </button>
            </div>
          </section>
        ) : (
          <div className="fus-stage" data-phase={phase}>
            <article
              className="fus-result-page"
              aria-labelledby="fus-result-title"
              data-focus={focusType ?? undefined}
            >
              <p className="fug-eyebrow">{t("firstUse.sample.pageEyebrow")}</p>
              <h2 id="fus-result-title">{page.title}</h2>
              <p className="fus-result-subtitle">{page.subtitle}</p>
              <div className="fus-result-rule" aria-hidden="true" />
              {page.paragraphs.map((paragraph) => {
                const lit = focusType != null &&
                  paragraphSources(paragraph).some((source) => source.type === focusType);
                return (
                  <Fragment key={paragraph.text}>
                    {paragraph.heading ? (
                      <h3 className="fus-result-heading">{paragraph.heading}</h3>
                    ) : null}
                    <p
                      className="fus-result-paragraph"
                      data-lit={lit}
                      data-dimmed={focusType != null && !lit}
                    >
                      {lit ? <mark>{paragraph.text}</mark> : paragraph.text}{" "}
                      {paragraph.citationIds.map((id) => {
                        const entry = citationById(id);
                        const source = entry ? sourceById(entry.sourceId) : undefined;
                        if (!entry || !source) return null;
                        return (
                          <button
                            key={id}
                            type="button"
                            data-testid={`sample-citation-${id}`}
                            data-source-type={source.type}
                            className="fus-citation"
                            aria-haspopup="dialog"
                            aria-label={source.title}
                            onClick={() => openSource(source.id, id)}
                          >
                            <SourceGlyph type={source.type} />
                            {source.kind}
                          </button>
                        );
                      })}
                    </p>
                  </Fragment>
                );
              })}
            </article>

            {phase < FINAL_PHASE ? (
              <section className="fus-origins" aria-label={t("firstUse.sample.sourcesTitle")}>
                <p className="fus-rack-heading">{t("firstUse.sample.sourcesTitle")}</p>
                <div className="fus-origin-grid">
                  {sources.map((source) => {
                    const lit = focusType === source.type;
                    return (
                      <button
                        key={source.id}
                        type="button"
                        data-testid={`sample-source-${source.id}`}
                        data-source-type={source.type}
                        data-lit={lit}
                        data-dimmed={focusType != null && !lit}
                        className="fus-rack-card"
                        aria-haspopup="dialog"
                        onClick={() =>
                          openSource(
                            source.id,
                            citations.find((entry) => entry.sourceId === source.id)?.id ?? null,
                          )
                        }
                      >
                        <span className="fus-source-icon" aria-hidden="true">
                          <SourceGlyph type={source.type} />
                        </span>
                        <span className="fus-rack-copy">
                          <span className="fus-source-kind">{`${source.kind} · ${source.when}`}</span>
                          <span className="fus-rack-title">{source.title}</span>
                          {quotesFor(source).map((quote) => (
                            <span key={quote} className="fus-rack-point">
                              {lit ? <mark>{quote}</mark> : quote}
                            </span>
                          ))}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </section>
            ) : (
              <section className="fus-chat" aria-labelledby="fus-chat-title">
                <div className="fus-chat-heading">
                  <p className="fus-rack-heading" id="fus-chat-title">
                    {t("firstUse.sample.aiTitle")}
                  </p>
                  <button
                    type="button"
                    className="fug-button-secondary"
                    onClick={() => setAiOpen(true)}
                  >
                    {t("firstUse.sample.useWithAi")}
                    <ArrowRight aria-hidden="true" weight="regular" />
                  </button>
                </div>
                <div className="fus-chat-turn" data-role="user">
                  <span className="fus-chat-who">{t("firstUse.sample.aiYou")}</span>
                  <p>{ai.question}</p>
                </div>
                <div className="fus-chat-turn" data-role="ai">
                  <span className="fus-chat-who">{t("firstUse.sample.aiName")}</span>
                  <p>{ai.answer}</p>
                </div>
                {/* Bridge from the sample to the user's own library. */}
                <div className="fus-own">
                  <p>{t("firstUse.sample.ownBody")}</p>
                  <button type="button" className="fug-button-primary" onClick={onFinish}>
                    {finishLabel ?? t("firstUse.sample.startOwn")}
                  </button>
                </div>
              </section>
            )}
          </div>
        )}
      </div>

      {dialog?.kind === "source" && dialogSource ? (
        <SampleDialog
          labelId="fus-citation-title"
          onClose={closeDialog}
          testId="sample-citation-dialog"
          sourceType={dialogSource.type}
          restoreFocusTo={dialogTrigger.current}
        >
          <div className="fus-dialog-header">
            <div>
              <p className="fug-eyebrow">{t("firstUse.sample.citationTitle")}</p>
              <h2 id="fus-citation-title">{dialogSource.title}</h2>
              <p className="fus-dialog-origin">
                {dialogSource.type === "file" && dialogSource.path != null
                  ? t("firstUse.sample.fileOrigin", {
                      path: dialogSource.path,
                      line: dialogSource.quoteLine ?? 1,
                    })
                  : t("firstUse.sample.memoryOrigin")}
              </p>
            </div>
            <button
              type="button"
              className="fus-icon-button"
              onClick={closeDialog}
              aria-label={t("firstUse.sample.close")}
            >
              <X aria-hidden="true" weight="regular" />
            </button>
          </div>
          {dialogCitation ? (
            <blockquote className="fus-quote">{dialogCitation.quote}</blockquote>
          ) : null}
          <div className="fus-original">
            {dialogSource.body.map((line, index) => (
              <p
                key={`${dialogSource.id}-${index}`}
                data-cited={dialogCitation ? line.includes(dialogCitation.quote) : false}
              >
                {dialogSource.type === "file" ? (
                  <span className="fus-line-number" aria-hidden="true">
                    {index + 1}
                  </span>
                ) : null}
                {/* One flex item, so a quote inside the line stays in the sentence. */}
                <span>
                  {dialogCitation ? (
                    <HighlightedLine line={line} quote={dialogCitation.quote} />
                  ) : (
                    line
                  )}
                </span>
              </p>
            ))}
          </div>
        </SampleDialog>
      ) : null}

    </>
  );
}
